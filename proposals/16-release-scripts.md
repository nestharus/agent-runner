# WU-16-01 Proposal — release-scripts

Phase: 3 proposal

Ticket: `tmp/scratch/wu-16-01/ticket.md` / source
`git show tickets/phase-16:plans/tickets/phase-16/WU-16-01.md`

Problem map: `research/16-release-scripts-problem-map.md`

## 1. Anti-scope (lock first)

Ticket Anti-scope, verbatim:

- Do NOT change the binary upload step's platform-suffix naming
  (WU-13-01 contract).
- Do NOT introduce script-versioning logic in the runner (e.g.,
  refusing to ingest if scripts are too old). v1 is "user installs
  matched versions"; runtime checks are a separate concern.
- Do NOT introduce backwards-compatibility shims for stale-script
  detection.
- Do NOT modify scripts themselves — they're correct as-shipped in
  WU-15-01.

Source: `tmp/scratch/wu-16-01/ticket.md:123-133`.

Proposal-derived anti-scope:

- Bare-binary platform-suffix contract (WU-13-01) is UNCHANGED. The
  Linux/macOS bare binaries continue to be collected as
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}`, and the Windows
  bare binary continues to be collected as
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe`.
  Source: `.github/workflows/release.yml:139-158`,
  `src-tauri/tests/release_yml_contract.rs:48-76`,
  `src-tauri/tests/release_yml_contract.rs:105-123`,
  `src-tauri/tests/release_yml_contract.rs:152-170`, and
  `src-tauri/tests/release_yml_contract.rs:264-273`.
- `.deb` / `.dmg` / `.msi` Tauri bundle contents are UNCHANGED. This proposal
  does not add adapter scripts to platform installers or Tauri bundle config.
  Source: ticket out-of-scope at `tmp/scratch/wu-16-01/ticket.md:95-99`;
  current collect steps at `.github/workflows/release.yml:139-158`.
- The seven adapter scripts themselves are UNCHANGED. They are release assets
  only; their bodies, interpreter declarations, behavior, and contracts are not
  edited. Source: ticket anti-scope at `tmp/scratch/wu-16-01/ticket.md:130-133`;
  script heads at `scripts/claude-code-turns:1-4`,
  `scripts/codex-turns:1-5`, `scripts/anthropic-usage:1-5`,
  `scripts/chatgpt-usage:1-5`, `scripts/zai-usage:1-5`,
  `scripts/claude-code-locate-transcript:1-5`, and
  `scripts/codex-locate-transcript:1-5`.
- `scripts/migrate-model-names.sh`, `scripts/tests/`, and `scripts/README.md`
  as a release asset are NOT uploaded. `scripts/README.md` may receive a
  one-line cross-reference, but it is not part of the AC-1 asset list.
  Source: ticket AC-1 at `tmp/scratch/wu-16-01/ticket.md:42-53`;
  problem map exclusions at `research/16-release-scripts-problem-map.md:261-279`.
- Frontend (`src/`) and Rust runtime (`src-tauri/src/`) are UNCHANGED. This WU
  is a release-CI/install-doc surface change, not a runtime ingestion, quota,
  migration, routing, or UI change.
  Source: ticket code boundary at `tmp/scratch/wu-16-01/ticket.md:81-107`;
  problem map scope exclusions at
  `research/16-release-scripts-problem-map.md:288-296`.
- Runtime version-skew detection is DEFERRED. The supported v1 path is that
  users install scripts and binary from the same release tag.
  Source: ticket anti-scope at `tmp/scratch/wu-16-01/ticket.md:127-131`.
- Adding scripts to the system PATH automatically is OUT OF SCOPE. Installation
  remains an explicit user action through the README snippet.
  Source: ticket out-of-scope at `tmp/scratch/wu-16-01/ticket.md:100-103`.
- A `scripts.tar.gz` release bundle is rejected for this proposal. Individual
  files match the ticket recommendation, preserve direct `gh release download`
  patterns, and match the WU-13-01 explicit release-asset pattern.
  Source: ticket notes at `tmp/scratch/wu-16-01/ticket.md:135-142`;
  problem map open question at `research/16-release-scripts-problem-map.md:459-466`.

## 2. Supported-surface track

Deployment mode:

- The deployment is the release-CI flow plus the user-install flow.
- Release CI is the manual `workflow_dispatch` workflow. It runs lint, test,
  version, build, then release; the release job checks out the repo, downloads
  artifacts, creates/pushes the tag, and invokes `softprops/action-gh-release@v2`.
  Source: `.github/workflows/release.yml:3-18`,
  `.github/workflows/release.yml:23-181`.
- User install is documented through README release binary installation and
  adapter script installation. Source: current release binary pointer at
  `README.md:7-10`; source-build manual install at `README.md:46-51`;
  current adapter install snippet at `README.md:332-350`.

Customer cohort:

- The directly broken cohort is binary-install users who get a body-aware
  `oulipoly-agent-runner` binary from a release but do not receive the matching
  adapter scripts. The ticket records that v0.1.26 shipped the body-aware
  binary without updated scripts, and stale local scripts do not emit `body`.
  Source: `tmp/scratch/wu-16-01/ticket.md:10-21`,
  `tmp/scratch/wu-16-01/ticket.md:24-30`.
- Source-build users remain supported because the existing repo-checkout
  install snippet remains valid and adjacent.
  Source: AC-3 at `tmp/scratch/wu-16-01/ticket.md:60-68`;
  current snippet at `README.md:340-350`.

Adjacent public/user-reachable paths:

- Release page asset list: newly includes the seven adapter scripts as direct
  release assets on the tag.
- `gh release download`: users can fetch the scripts from the release asset
  list by name and put them on `~/.local/bin`.
  Source: GitHub CLI manual for `gh release download` tag argument, `--dir`,
  `--pattern`, and multiple pattern examples at
  https://cli.github.com/manual/gh_release_download lines 513-547.
- Existing source-build install: unchanged `install -m 755 scripts/...`
  snippet remains valid for users installing from a checkout.
  Source: `README.md:340-350`.

Blast-radius notes for unchanged adjacent paths:

- Every build job and platform collect path stays the same: Linux `.deb` plus
  target-suffixed bare binary; macOS `.dmg` plus target-suffixed bare binary;
  Windows `.msi`, NSIS `.exe`, and target-suffixed bare `.exe`.
  Source: `.github/workflows/release.yml:100-162`.
- `actions/upload-artifact@v4` and `actions/download-artifact@v4` stay in the
  same upload/download roles. The release job still downloads matrix artifacts
  into `artifacts/`, and `artifacts/*` remains part of the release upload list.
  Source: `.github/workflows/release.yml:159-172`.
- The WU-13-01 structural assertions stay active and are extended, not replaced.
  Source: `src-tauri/tests/release_yml_contract.rs:14-273`;
  prior WU-13 test-intent precedent at `proposals/13-release-restore.md:582-616`.
- Runtime routes, quota scoring, state DB migrations, ingestion readers, and
  frontend views are not reached by this change. Source: ticket out-of-scope at
  `tmp/scratch/wu-16-01/ticket.md:95-107`.

Release CI surface:

- Change: the `softprops/action-gh-release@v2` `with.files` input changes from
  the single scalar `artifacts/*` to a YAML block scalar containing
  `artifacts/*` plus seven explicit `scripts/<name>` entries.
  Source: current edit point at `.github/workflows/release.yml:177-181`;
  problem map single edit point at
  `research/16-release-scripts-problem-map.md:34-38`.
- Change: `src-tauri/tests/release_yml_contract.rs` extends its structural
  assertion around the existing `gh_release` lookup to require each adapter
  script entry in `with.files`.
  Source: existing lookup/assertion at
  `src-tauri/tests/release_yml_contract.rs:253-262`;
  helper surface at `src-tauri/tests/release_yml_contract.rs:320-377`.
- Stays the same: workflow trigger, permissions, lint/test/version/build jobs,
  matrix rows, Tauri build, platform collect steps, upload-artifact step,
  download-artifact step, tag creation step, and generated release notes.
  Source: `.github/workflows/release.yml:3-181`.

User-install surface:

- README keeps the source-build snippet in `§Reference quota adapters`:

```bash
install -m 755 \
  scripts/anthropic-usage \
  scripts/chatgpt-usage \
  scripts/zai-usage \
  scripts/claude-code-turns \
  scripts/codex-turns \
  ~/.local/bin/
```

  Source: current source-build snippet at `README.md:340-350`.

- README appends a binary-install note immediately after that snippet and before
  `## Session Ingestion` at `README.md:352`. The snippet should be exactly this
  shape, with the version placeholder retained:

```bash
gh release download v0.1.X --repo nestharus/agent-runner \
  --pattern "claude-code-turns" --pattern "codex-turns" \
  --pattern "anthropic-usage" --pattern "chatgpt-usage" \
  --pattern "zai-usage" \
  --pattern "claude-code-locate-transcript" \
  --pattern "codex-locate-transcript" \
  --dir ~/.local/bin/
chmod +x ~/.local/bin/{claude-code-turns,codex-turns,anthropic-usage,chatgpt-usage,zai-usage,claude-code-locate-transcript,codex-locate-transcript}
```

  Source: ticket suggested snippet at `tmp/scratch/wu-16-01/ticket.md:143-154`;
  GitHub CLI options at https://cli.github.com/manual/gh_release_download
  lines 513-547.

- The note must say: install adapter scripts from the same release tag as the
  binary; mismatched stale scripts may silently omit `body`, leaving
  `session_turns.body` empty for new ingests.
  Source: AC-3 at `tmp/scratch/wu-16-01/ticket.md:60-68`;
  symptom at `tmp/scratch/wu-16-01/ticket.md:24-30`.

Migration path:

- None required. This is additive on release assets and documentation.
- Legacy installers can remain on the source-build snippet until they choose to
  switch to release asset downloads. Source-build remains documented and valid.
  Source: `README.md:340-350`; AC-3 at `tmp/scratch/wu-16-01/ticket.md:60-68`.

Rollback path:

- Revert the publish-step `files:` block to the prior `artifacts/*` shape.
- Revert the structural test extension that expects the seven script entries.
- Revert the README release-asset snippet and optional `scripts/README.md`
  one-line cross-reference.
- There is no DB, runtime migration, or package-manager coupling.
  Source: code boundary limited to release YAML, test, README, optional
  `scripts/README.md` at `tmp/scratch/wu-16-01/ticket.md:81-93`.

Observability:

- No runtime observability is required because the app runtime is unchanged.
- The visible deployment signal is the GitHub release page asset list and the
  `softprops/action-gh-release@v2` upload output for the release job.
- Merge-time observability is the structural test failing if the publish step
  stops listing any required adapter path.
  Source: current structural-test gate at
  `src-tauri/tests/release_yml_contract.rs:6-18`,
  `src-tauri/tests/release_yml_contract.rs:253-262`;
  softprops `assets` output documentation at
  https://github.com/softprops/action-gh-release/blob/v2/action.yml lines 75-76.

## 3. Assumption register (approved)

A1 — `softprops/action-gh-release@v2` accepts a newline-delimited `files`
input containing direct file paths and globs.

- Statement: The release step can use a YAML block scalar for `with.files`
  containing `artifacts/*` plus explicit `scripts/<name>` paths.
- Evidence: current workflow already uses `files: artifacts/*` at
  `.github/workflows/release.yml:177-181`; softprops v2 README documents
  `with.files` as a newline-delimited list of glob expressions and says files
  can be listed directly by name at
  https://github.com/softprops/action-gh-release/tree/v2 lines 355-403;
  v2 action metadata defines `files` as a newline-delimited list of path globs
  at https://github.com/softprops/action-gh-release/blob/v2/action.yml
  lines 27-29.
- Falsification path: the exact `softprops/action-gh-release@v2` revision used
  by GitHub Actions rejects a block scalar or direct paths, or its metadata no
  longer exposes `files` as a string input.
- Owner: Phase 4 validates as a supported-surface/risk assumption; Phase 5
  rechecks against hookpoint details before implementation.

A2 — Direct `scripts/<name>` inputs upload assets whose downloadable asset names
are the basename script names, with no extension changes.

- Statement: Listing `scripts/claude-code-turns` uploads an asset named
  `claude-code-turns`; the `scripts/` prefix is the workflow path, not a desired
  slash-bearing GitHub asset name. This tightens the problem-map draft:
  "preserve filenames" means no extension or rename of the seven script names,
  matching the ticket examples and README `gh release download --pattern`
  names.
- Evidence: ticket AC-1 lists the path family as `scripts/` and examples as
  bare names at `tmp/scratch/wu-16-01/ticket.md:42-53`; ticket README snippet
  downloads by bare asset name at `tmp/scratch/wu-16-01/ticket.md:146-153`;
  softprops v2 source constructs release asset metadata with `name:
  basename(path)` and upload query `name` from that basename at
  https://github.com/softprops/action-gh-release/blob/v2/src/github.ts
  lines 257-263 and 304-335.
- Falsification path: a trial release, upstream action source, or GitHub API
  behavior shows that `scripts/<name>` becomes a different downloadable name,
  or that slash-bearing release asset names are required by AC-1.
- Owner: Phase 4 flags any interpretation conflict with AC-1; Phase 5 binds
  the exact test assertion; Phase 6b validates against trial-release asset
  names if a trial release is run.

A3 — All seven AC-1 adapter scripts live under `scripts/` at HEAD and have
stable names.

- Statement: The release asset set is exactly:
  `scripts/claude-code-turns`, `scripts/codex-turns`,
  `scripts/anthropic-usage`, `scripts/chatgpt-usage`, `scripts/zai-usage`,
  `scripts/claude-code-locate-transcript`, and
  `scripts/codex-locate-transcript`.
- Evidence: ticket AC-1 names the seven assets at
  `tmp/scratch/wu-16-01/ticket.md:42-53`; script heads exist at
  `scripts/claude-code-turns:1-4`, `scripts/codex-turns:1-5`,
  `scripts/anthropic-usage:1-5`, `scripts/chatgpt-usage:1-5`,
  `scripts/zai-usage:1-5`, `scripts/claude-code-locate-transcript:1-5`,
  and `scripts/codex-locate-transcript:1-5`.
- Falsification path: any of the seven files is renamed, deleted, moved, or
  replaced with a non-script artifact before Phase 6 implementation.
- Owner: Phase 5 verifies before hookpoint binding; Phase 6b verifies through
  the structural test and file existence during implementation.

A4 — The structural release-yml test can be extended without replacing the
WU-13-01 release contract assertions.

- Statement: AC-2 is an additive assertion against the existing parsed
  `softprops/action-gh-release@v2` step, not a rewrite of the test or a
  weakening of bare-binary/bundle checks.
- Evidence: test reads and parses the workflow at
  `src-tauri/tests/release_yml_contract.rs:6-18`; existing publish-step lookup
  and assertion are at `src-tauri/tests/release_yml_contract.rs:253-262`;
  helper functions for step lookup and YAML value access are at
  `src-tauri/tests/release_yml_contract.rs:320-377`; WU-13 release-flow
  structural invariants are documented at `proposals/13-release-restore.md:582-616`.
- Falsification path: converting `with.files` to a block scalar is not parsed as
  `Value::String` by `serde_yml`, or the clean assertion requires deleting
  existing WU-13 assertions.
- Owner: Phase 4 reviews scope/test risk; Phase 5 confirms exact assertion
  shape; Phase 6b compiles and runs the test.

A5 — README has a stable primary install-doc insertion point adjacent to the
source-build adapter snippet.

- Statement: The binary-install `gh release download` note should be appended
  immediately after the existing `§Reference quota adapters` source-build
  snippet, leaving the source-build snippet intact.
- Evidence: current subsection and source-build snippet are at
  `README.md:332-350`; `## Session Ingestion` begins at `README.md:352`; AC-3
  requires the source-build snippet to remain valid and a binary-install note
  to be added at `tmp/scratch/wu-16-01/ticket.md:60-68`.
- Falsification path: Phase 5 finds that the README section has moved or a more
  accurate equivalent install section exists that avoids splitting source-build
  and binary-install instructions.
- Owner: Phase 4 reviews supported-surface clarity; Phase 5 binds exact lines;
  Phase 6b applies the doc edit.

A6 — `scripts/README.md` can receive a one-line cross-reference without
becoming a second install procedure.

- Statement: A single sentence can point users to README
  `§Reference quota adapters` for release-asset installation while leaving
  `scripts/README.md` focused on adapter contracts.
- Evidence: `scripts/README.md` defines adapter scripts as standalone
  executables wired through TOML at `scripts/README.md:1-5`; it documents turn
  scripts, locators, and quota scripts at `scripts/README.md:70-81`,
  `scripts/README.md:139-145`, and `scripts/README.md:196-242`; ticket marks
  this file optional at `tmp/scratch/wu-16-01/ticket.md:91-93`.
- Falsification path: reviewers determine the cross-reference creates a second
  install surface, duplicates the README, or distracts from adapter contracts.
- Owner: Phase 4 scope/supported-surface reviewers decide if optional doc scope
  stands; Phase 5 binds the insertion point only if accepted.

## 4. Test-intent track

AC-1 — seven scripts are uploaded as release assets.

- Change risk or verification risk: the publish step may continue uploading
  only `artifacts/*`, or it may use a broad `scripts/*` glob that accidentally
  includes `scripts/README.md`, `scripts/tests/`, or future non-adapter scripts.
- Intended behavior: `softprops/action-gh-release@v2` `with.files` contains
  `artifacts/*` and exactly the seven explicit adapter script paths required by
  AC-1.
- Selected level: particular-integration structural test against workflow YAML.
- Fixture source / application point: `.github/workflows/release.yml` parsed by
  `src-tauri/tests/release_yml_contract.rs`; publish step located by
  `step_by_uses(release_steps, "softprops/action-gh-release@v2")`.
  Source: `src-tauri/tests/release_yml_contract.rs:228-262`,
  `src-tauri/tests/release_yml_contract.rs:327-332`.
- Assumption links: A1, A2, A3, A4.
- Expected observable signal: test fails unless each of these exact strings is
  present as a release-file entry:
  `scripts/claude-code-turns`, `scripts/codex-turns`,
  `scripts/anthropic-usage`, `scripts/chatgpt-usage`, `scripts/zai-usage`,
  `scripts/claude-code-locate-transcript`, and
  `scripts/codex-locate-transcript`.
- Assertion shape: parse `with.files` as a `Value::String`, split lines with
  `files.lines().map(str::trim).filter(|line| !line.is_empty())`, collect into
  `BTreeSet<String>`, and use string equality against a set containing
  `artifacts/*` plus the seven explicit script paths. This is stricter than
  regex and stricter than glob matching; it prevents accidental broad upload of
  `scripts/*` or omitted scripts while preserving order independence.
- Residual risk: structural YAML cannot prove GitHub's release page contains
  uploaded assets after a live release; if Phase 6b does not run a trial
  release, it must write `risk/16-release-scripts-test-residuals.md` naming
  that unverified live-release residual.

AC-2 — trial release publishes all expected scripts and structural test extends
WU-13 surface.

- Change risk or verification risk: a Phase 6 diff could satisfy AC-1 locally
  but weaken WU-13 bare-binary assertions or fail to produce assets in a live
  release.
- Intended behavior: the structural test is extended in
  `src-tauri/tests/release_yml_contract.rs`; it still asserts matrix rows,
  bundle globs, target-suffixed bare binaries, artifact upload/download, and
  release files.
- Selected level: particular-integration structural test, plus manual
  release-CI evidence if a trial release is available.
- Fixture source / application point: same file and same parsed workflow as the
  existing `release_yml_restores_windows_and_target_suffixed_bare_binaries`
  test. Source: `src-tauri/tests/release_yml_contract.rs:6-18`,
  `src-tauri/tests/release_yml_contract.rs:14-273`.
- Assumption links: A1, A2, A4.
- Expected observable signal: `cargo test` includes the release YAML contract
  test passing on the new `files:` block; a trial release asset list, when run,
  shows all seven basename assets.
- Test placement: prefer extending the existing
  `release_yml_restores_windows_and_target_suffixed_bare_binaries` test near
  `src-tauri/tests/release_yml_contract.rs:253-262`, because it already owns the
  publish-step contract and keeps AC-6 non-regression assertions adjacent. A
  sibling test in the same file is acceptable only if Phase 5 finds the existing
  test becomes too dense; either way, helper reuse and assertions remain
  additive.
- Residual risk: if no trial release is run, Phase 6b must write
  `risk/16-release-scripts-test-residuals.md` describing the live-release
  evidence gap and the structural coverage that remains.

AC-3 — README documents binary-install script download while preserving
source-build snippet.

- AC-3 is documentation and is not test-encoded.
- Change risk or verification risk: users could still miss that binary installs
  need matching scripts from the same release tag, or the source-build snippet
  could be displaced.
- Intended behavior: README keeps the current source-build install snippet and
  appends the `gh release download` plus `chmod +x` binary-install snippet
  immediately after it. The note explicitly names matched binary/script release
  versions as required for body ingestion.
- Selected level: documentation review only.
- Fixture source / application point: `README.md:332-350`, before
  `README.md:352`.
- Assumption links: A5.
- Expected observable signal: README contains both install paths in one local
  area, with no split across multiple primary docs.
- Residual risk: no automated doc test verifies command freshness. The command
  syntax is grounded in ticket notes and GitHub CLI docs; future GitHub CLI
  changes are outside this WU.

AC-4 — existing CI stays green and structural release contract test passes.

- Change risk or verification risk: converting `files:` to a block scalar could
  break the Rust YAML parser assertion or an unrelated CI gate.
- Intended behavior: the release contract test parses the YAML and passes on
  the new `files:` string; existing CI workflow remains green.
- Selected level: unit/particular-integration as CI currently runs them.
- Fixture source / application point: `src-tauri/tests/release_yml_contract.rs`
  and `.github/workflows/release.yml`.
- Assumption links: A1, A4.
- Expected observable signal: `cargo test` passes locally/CI; release-yml
  contract failure messages identify missing or extra release-file entries.
- Residual risk: this does not prove GitHub Actions release execution unless a
  workflow run is performed. Residual risk path:
  `risk/16-release-scripts-test-residuals.md` if release execution is not
  externally evidenced.

AC-5 — cargo fmt, clippy, cargo test, and frontend gates stay green on Linux
and macOS in CI.

- Change risk or verification risk: Rust test edits could require formatting or
  clippy cleanup; docs/YAML edits should not affect frontend, but CI must remain
  green.
- Intended behavior: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test --no-fail-fast`, and frontend gates continue to pass.
- Selected level: CI suite / unit and particular-integration tests.
- Fixture source / application point: release contract test extension under
  `src-tauri/tests/`; unchanged frontend gates in release workflow lint/test
  jobs. Source: `.github/workflows/release.yml:23-67`.
- Assumption links: A4.
- Expected observable signal: CI passes on Linux and macOS per AC-5.
- Residual risk: Phase 3 cannot encode platform CI success; Phase 6b reports
  any command not run or not available. Residual risk path:
  `risk/16-release-scripts-test-residuals.md` if platform CI success is not
  externally evidenced.

AC-6 — existing release workflow paths continue to publish.

- Change risk or verification risk: adding script assets could accidentally
  remove `artifacts/*`, change artifact aggregation, or weaken WU-13
  platform-suffix checks.
- Intended behavior: `artifacts/*` remains a release-file entry, upload/download
  remains unchanged, collect steps remain unchanged, and WU-13 bare-binary /
  bundle assertions continue to pass.
- Selected level: particular-integration structural test.
- Fixture source / application point: existing assertions in
  `src-tauri/tests/release_yml_contract.rs:14-273`, especially upload/download
  at `:208-251`, publish files at `:253-262`, and bare-binary hit scan at
  `:264-273`.
- Assumption links: A1, A4.
- Expected observable signal: equality assertion on release-file entries still
  includes `artifacts/*`, and all existing matrix/bundle/bare-binary assertions
  remain in place.
- Residual risk: structural test cannot prove actual `.deb`, `.dmg`, `.msi`,
  NSIS `.exe`, and bare binaries are produced in a live release run; WU-13
  precedent already treats live release evidence separately from structural
  merge-time coverage. Source: `proposals/13-release-restore.md:582-616` and
  `risk/13-release-restore-supported-surface.md:110-129`. Residual risk path:
  `risk/16-release-scripts-test-residuals.md` if live release evidence does not
  cover these assets.

Residual-risk artifact requirement:

- If Phase 6b cannot encode any AC above, or if no live trial release is run for
  AC-2, the test writer must produce
  `risk/16-release-scripts-test-residuals.md`.

## 5. Qualitative net-value statement

This proposal reduces a concrete current-state risk on the current supported
surface: WU-15-01 install QA found that binary-install users could receive a
body-aware binary without matching body-aware adapter scripts, leaving
`session_turns.body` empty until a manual repo-copy update.
Source: `tmp/scratch/wu-16-01/ticket.md:10-30`;
WU-15 body-aware adapter proposal precedent at `proposals/15-empty-bodies-ref.md:73-80`;
WU-15 problem-map evidence that body capture depended on adapter output at
`research/15-empty-bodies-ref-problem-map.md:86-99`.

The reduction outweighs the added blast radius: one YAML edit point
(`.github/workflows/release.yml:177-181`), one structural test extension
(`src-tauri/tests/release_yml_contract.rs:253-262`), one README snippet
(`README.md:332-352`), and one optional `scripts/README.md` cross-reference
(`scripts/README.md:1-5`). The unchanged adjacent surfaces remain covered by
the WU-13 release-flow precedent at `research/13-release-restore-problem-map.md:211-220`,
`research/13-release-restore-hookpoints.md:455-551`,
`proposals/13-release-restore.md:254-274`, and
`risk/13-release-restore-supported-surface.md:110-129`.

Supported-surface termination check: no approved assumption is currently
invalidated, and the value on the supported surface is positive because binary
install users gain a direct matched-version script install path while release
CI, bundle packaging, and runtime behavior remain unchanged.

## 6. Implementation outline (design-level only — do NOT write code)

Publish-step extension:

- Choose Option A: append explicit `scripts/<name>` entries to the
  `softprops/action-gh-release@v2` `files:` list.
- The publish step's `files:` input gains an explicit list with one entry per
  ticket AC-1 adapter, in addition to the existing `artifacts/*` entry. The
  seven exact script paths are named in the §4 AC-1 test-intent track, and the
  bare-binary contract from WU-13-01 stays unchanged.
- This uses the release job's existing `actions/checkout@v4`, so repo content
  under `scripts/` is available directly. Source:
  `.github/workflows/release.yml:168-181`.
- It also preserves the current `actions/download-artifact@v4` and
  `artifacts/*` path for Tauri bundles and bare binaries. Source:
  `.github/workflows/release.yml:168-181`.

Trade-offs vs Option B:

- Option A keeps a single release `files:` list as the source of truth. The
  structural test can assert exact string entries in that list.
- Option A avoids another staging/copy step in the release job and avoids
  changing artifact aggregation from build jobs.
- Option A matches the explicit-list pattern WU-13-01 used for bare-binary
  output naming: asset names are intentionally visible in workflow structure
  and structurally asserted. Source: WU-13 proposal at
  `proposals/13-release-restore.md:254-274`;
  WU-13 hookpoints at `research/13-release-restore-hookpoints.md:515-551`.
- Option B, copying scripts into `artifacts/`, would leave `files: artifacts/*`
  unchanged but make the release job stage non-build repo files into the same
  directory as build outputs. That broadens the staging surface and makes the
  structural assertion indirect.

Structural-test extension shape:

- Reuse the current test file and helper style in
  `src-tauri/tests/release_yml_contract.rs`.
- Reuse the workflow read and parse setup at `src-tauri/tests/release_yml_contract.rs:6-18`.
- Reuse `step_by_uses` at `src-tauri/tests/release_yml_contract.rs:327-332`
  to locate `softprops/action-gh-release@v2`.
- Reuse `string_at` at `src-tauri/tests/release_yml_contract.rs:341-346`
  to read `with.files` as a string.
- Update the current single-value publish-file assertion at
  `src-tauri/tests/release_yml_contract.rs:253-262` into a structural
  assertion over the non-empty trimmed release-file entries.
- The structural-test extension asserts that the publish step's parsed
  `files:` value contains every ticket AC-1 script path, asserted by
  exact-string set membership using the existing YAML-parsing helpers, without
  relying on regex or glob inference. The exact path set and assertion
  technique remain specified in §4 AC-1.
- Keep the assertion inside
  `release_yml_restores_windows_and_target_suffixed_bare_binaries` unless
  Phase 5 finds the existing test too dense; a sibling test in the same file is
  acceptable only if it still reuses helper functions and leaves existing
  WU-13 assertions intact.

README snippet:

- Insert immediately after `README.md:340-350` and before `README.md:352`.
- Preserve the source-build snippet exactly. The primary AC-3 requirement is
  that it remains valid, not that it is replaced or expanded for parity with
  the release-asset snippet.
- Add a paragraph stating that binary installs can download adapter scripts
  from the matching GitHub release tag and that scripts and binary versions
  must match for body ingestion; stale scripts may omit `body` silently.
- Add the binary-install command shape described in §2, preserving the version
  placeholder, all seven download patterns, and the executable-permission step.

Optional `scripts/README.md` cross-reference:

- Recommend including it.
- Add one sentence near the opening description at `scripts/README.md:1-5`:
  "For release-asset installation of the bundled reference adapters, see
  README §Reference quota adapters."
- Do not add a second install command block to `scripts/README.md`.

Platform suffixes / script portability:

- The release asset names have no platform suffixes and no extension changes.
  This matches AC-1's seven names and the ticket's `gh release download`
  patterns. Source: `tmp/scratch/wu-16-01/ticket.md:42-53`,
  `tmp/scratch/wu-16-01/ticket.md:146-153`.
- As shipped, all seven scripts are Unix-style executable scripts:
  `claude-code-turns` and `codex-turns` use `#!/usr/bin/env python3`; the
  quota scripts and transcript locators use `#!/usr/bin/env bash`, with locator
  scripts delegating to embedded Python. Source: `scripts/claude-code-turns:1-4`,
  `scripts/codex-turns:1-5`, `scripts/anthropic-usage:1-5`,
  `scripts/chatgpt-usage:1-5`, `scripts/zai-usage:1-5`,
  `scripts/claude-code-locate-transcript:1-5`, and
  `scripts/codex-locate-transcript:1-5`.
- Linux and macOS users can run the scripts as named after `chmod +x` when
  `bash` and `python3` are available.
- Windows release binaries continue to publish unchanged, but these adapter
  scripts are not Windows `.exe`, `.ps1`, or `.bat` assets. Windows users need a
  POSIX-like shell plus Python, such as WSL or Git Bash, for these scripts as
  shipped. This proposal does not add Windows-specific script wrappers.

## 7. Open questions for Phase 5

1. Confirm the exact publish-step assertion helper shape:
   whether to add a small local helper such as `release_files(gh_release)` or
   inline the line-splitting in the existing test. The assertion semantics are
   already fixed as set equality over trimmed non-empty `with.files` lines.
   Source: `src-tauri/tests/release_yml_contract.rs:253-262`,
   `src-tauri/tests/release_yml_contract.rs:341-346`.

2. Confirm whether the script release-file assertion stays inside
   `release_yml_restores_windows_and_target_suffixed_bare_binaries` or becomes
   a sibling test in the same file. Proposal preference is inside the existing
   test because publish-step ownership is already there.
   Source: problem map open question at
   `research/16-release-scripts-problem-map.md:479-483`.

3. Confirm whether `scripts/README.md` receives the optional one-line
   cross-reference. Proposal preference is yes, one line only, because the file
   already tells users that scripts are standalone executables not linked into
   the binary. Source: `scripts/README.md:1-5`;
   problem map open question at
   `research/16-release-scripts-problem-map.md:474-478`.

4. Confirm that Phase 5 hookpoint research still supports the Phase 3 choice
   of individual script assets through Option A, rather than a `scripts.tar.gz`
   bundle or an `artifacts/` staging step. The proposal already rejects the
   bundle and chooses explicit `scripts/<name>` release-file entries; Phase 5
   only needs to verify the hookpoint can carry that choice without widening
   scope. Source: problem map open questions at
   `research/16-release-scripts-problem-map.md:461-473`;
   ticket notes at `tmp/scratch/wu-16-01/ticket.md:135-158`.

5. Confirm the Phase 6b evidence plan for AC-2: if no trial release run is
   performed, create `risk/16-release-scripts-test-residuals.md` documenting
   that the structural test guards workflow shape but not live GitHub release
   asset materialization.

## 8. Round 2 changelog

- AUDIT-01 closure: removed the former §6 YAML `files: |` target block that
  Round 1 audit identified at lines 513-526, including the duplicated exact
  release-file list. The same exact seven paths remain preserved in §4 AC-1's
  expected observable signal and assertion shape, and the ticket AC-1
  cross-reference still binds the asset set.
- AUDIT-01 closure: removed the former §6 set-literal assertion block that
  Round 1 audit identified at lines 560-575. The exact assertion technique
  remains in §4 AC-1 as YAML parsing, trimmed non-empty line splitting,
  `BTreeSet<String>` collection, and string-equality set comparison against
  `artifacts/*` plus the seven script paths.
- AUDIT-01 closure: revised the §6 README command instruction from
  code-shaped "one-liner" language to prose that points back to the §2
  install-snippet shape. Rationale: §6 stays design-level only, with
  code-shape content moved to or preserved in §4 and §2 where those details
  belong.
- AUDIT-02 closure: added inline residual-risk artifact paths to AC-4, AC-5,
  and AC-6 adjacent to their residual statements, using
  `risk/16-release-scripts-test-residuals.md`.
- No other design, scope, anti-scope, supported-surface, assumption, net-value,
  or open-question content was changed.

Status: ready for Phase 4
