# WU-16-01 Release Scripts Problem Map

Phase: 2.5 existing-state risk profile

Pre-fix state: `bc6df8e fix(state): persist session turn bodies in state.db (#40)`.
The triggering WU-15-01 install QA finding is that v0.1.26 shipped a
body-aware `oulipoly-agent-runner` binary but did not ship the matching
adapter scripts as release assets; users who install binaries from the
release page can keep stale local scripts, so new `body` values never reach
`session_turns.body` unless the user manually clones the repo and copies the
scripts. The ticket records the missing assets, the stale-script symptom, and
the audited `.deb` package gap at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:8-22`
and `:24-38`.

This document maps the existing release-script surface only. Proposals are
not in scope for this phase.

## 1. Touched surface — files in scope

### `.github/workflows/release.yml`

- File role: release workflow touched by the ticket Code Boundary for adding
  script release assets. The ticket names this file as in-scope at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:81-90`.
- Current trigger: manual-only `workflow_dispatch`; no tag-push trigger is
  active in this workflow. `.github/workflows/release.yml:3-16`.
- Current release permission: `permissions.contents: write`, needed for tag
  creation and release upload. `.github/workflows/release.yml:17-18`.
- Publish job name: `release`. It depends on `version` and `build`, runs on
  `ubuntu-latest`, checks out the repo, downloads build artifacts, creates and
  pushes the version tag, then calls `softprops/action-gh-release@v2`.
  `.github/workflows/release.yml:164-181`.
- Publish step: the `softprops/action-gh-release@v2` step is currently
  anonymous, identified by `uses: softprops/action-gh-release@v2`.
  `.github/workflows/release.yml:177-181`.
- Current `files:` value: exactly `artifacts/*`; no script paths are listed.
  `.github/workflows/release.yml:177-181`.

Verbatim publish-job shape at HEAD:

```yaml
release:
  needs: [version, build]
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions/download-artifact@v4
      with:
        merge-multiple: true
        path: artifacts
    - name: Create tag
      run: |
        git tag ${{ needs.version.outputs.tag }}
        git push origin ${{ needs.version.outputs.tag }}
    - uses: softprops/action-gh-release@v2
      with:
        tag_name: ${{ needs.version.outputs.tag }}
        generate_release_notes: true
        files: artifacts/*
```

Observed lines: `.github/workflows/release.yml:164-181`.

- Artifact aggregation feeding the publish step is a build-job upload plus
  release-job download pattern. Each build matrix row creates an `artifacts`
  directory, copies platform output into it, and uploads `artifacts/*`.
  `.github/workflows/release.yml:139-162`.
- Linux build artifact staging currently copies a `.deb` bundle glob and the
  target-suffixed bare binary into `artifacts/`.
  `.github/workflows/release.yml:139-145`.
- macOS build artifact staging currently copies a `.dmg` bundle glob and the
  target-suffixed bare binary into `artifacts/`.
  `.github/workflows/release.yml:145-150`.
- Windows build artifact staging currently copies `.msi`, NSIS `.exe`, and the
  target-suffixed bare `.exe` into `artifacts/`.
  `.github/workflows/release.yml:151-158`.
- The per-platform upload step is `actions/upload-artifact@v4` with
  `name: ${{ matrix.target }}` and `path: artifacts/*`.
  `.github/workflows/release.yml:159-162`.
- The publish job uses `actions/download-artifact@v4` with
  `merge-multiple: true` and `path: artifacts`, flattening all uploaded matrix
  artifacts into the publish job's `artifacts/` directory before the
  `softprops` step consumes `artifacts/*`.
  `.github/workflows/release.yml:168-172`.
- If adapter scripts are made visible by the publish job, the two currently
  adjacent supported paths are: direct checkout paths under `scripts/` because
  the publish job does `actions/checkout@v4`, and staged paths under
  `artifacts/` because the publish job downloads build artifacts there.
  `.github/workflows/release.yml:168-181`.

### `src-tauri/tests/release_yml_contract.rs`

- File role: WU-13-01 structural test already asserts the release workflow's
  matrix, artifact collection, upload/download, and release files contract.
  The WU-16-01 ticket explicitly names this file as the AC-2 extension point.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:54-58`
  and `:108-115`.
- Existing test function:
  `release_yml_restores_windows_and_target_suffixed_bare_binaries`.
  `src-tauri/tests/release_yml_contract.rs:6-7`.
- Existing YAML parse shape: it reads `../.github/workflows/release.yml`,
  parses it with `serde_yml::from_str`, and then navigates `Value` with helper
  functions. `src-tauri/tests/release_yml_contract.rs:8-18` and `:334-377`.
- Existing matrix assertions: the test expects exactly three build matrix rows
  for Linux, macOS, and Windows with target/bundle triples:
  `x86_64-unknown-linux-gnu` + `deb`, `aarch64-apple-darwin` + `dmg`, and
  `x86_64-pc-windows-msvc` + `msi,nsis`.
  `src-tauri/tests/release_yml_contract.rs:14-46`.
- Existing bare-binary platform-suffix assertions:
  Linux, macOS, and Windows collect steps must target-suffix the bare binary,
  with Windows using `.exe`. `src-tauri/tests/release_yml_contract.rs:48-206`.
- Existing bundle-upload assertions:
  Linux must copy `bundle/deb/*.deb`; macOS must copy `bundle/dmg/*.dmg`;
  Windows must copy `bundle/msi/*.msi` and `bundle/nsis/*.exe`; each platform
  also has negative assertions against the other platforms' bundle substrings.
  `src-tauri/tests/release_yml_contract.rs:77-103`, `:124-150`, and `:171-206`.
- Existing build upload assertion: the `actions/upload-artifact@v4` step must
  use `name: ${{ matrix.target }}` and `path: artifacts/*` or `artifacts`.
  `src-tauri/tests/release_yml_contract.rs:208-226`.
- Existing release download assertion: the `actions/download-artifact@v4` step
  must use `merge-multiple: true` and `path: artifacts`.
  `src-tauri/tests/release_yml_contract.rs:228-251`.
- Existing publish-files assertion: the `softprops/action-gh-release@v2` step's
  `with.files` is currently asserted to equal exactly `artifacts/*`.
  `src-tauri/tests/release_yml_contract.rs:253-262`.
- Existing final bare-binary scan assertion: the target-suffixed bare binary
  substring must appear only in the three platform collect steps.
  `src-tauri/tests/release_yml_contract.rs:264-273`.
- Precise additive extension point for AC-2: directly after the existing
  `gh_release` lookup at `src-tauri/tests/release_yml_contract.rs:253-262`,
  the test can reuse the parsed `gh_release` value and inspect
  `jobs.release.steps[softprops/action-gh-release@v2].with.files` for the
  adapter script references, while preserving the existing matrix, bundle, and
  bare-binary assertions at `:14-273`. This is an extension point description,
  not a Phase 2.5 fix selection.

### `README.md`

- File role: ticket AC-3 names `README.md` for install-path documentation.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:60-68`.
- Existing nearby manual binary install section: `### Manual install
  (Linux/macOS)` copies the locally built binary into `~/.local/bin/`.
  `README.md:46-51`.
- Existing quota adapter subsection: `**Reference quota adapters** (in
  [scripts/](scripts/)):` appears under quota-script documentation.
  `README.md:332-339`.
- Existing source-build adapter install snippet begins with "Install them on
  your `$PATH`" and installs five scripts from the repo checkout:
  `anthropic-usage`, `chatgpt-usage`, `zai-usage`, `claude-code-turns`, and
  `codex-turns`. `README.md:340-350`.
- Neighboring section after that snippet is `## Session Ingestion`, which
  describes turn scripts, body ingestion, and `session_turns.body`.
  `README.md:352-390`.
- Existing "Reference turn adapters" subsection lists `claude-code-turns` and
  `codex-turns`, then points readers to `scripts/README.md` for custom scripts.
  `README.md:394-401`.
- Existing transcript locator subsection begins immediately after that at
  `### Optional: transcript_locator` and shows locator wiring.
  `README.md:403-410`.
- Existing configuration tree documents `sessions.toml` as the per-provider
  turn ingestion and transcript locator adapter config file.
  `README.md:721-730`.
- AC-3 insertion terrain is therefore the adapter-install documentation around
  `README.md:332-350` and the immediately adjacent session-ingestion material
  at `README.md:352-410`; the ticket requires the current source-build snippet
  to remain valid and a binary-install release-asset note to be added.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:60-68`.

### `scripts/README.md`

- File role: optional doc edit point only, per ticket Code Boundary.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:91-93`.
- Current purpose: it documents standalone adapter scripts wired through TOML;
  it explicitly says the scripts are not linked into the binary.
  `scripts/README.md:1-5`.
- Turn-script contract documents optional `body` emission and says to omit
  `body` when content cannot be extracted. `scripts/README.md:21-40`.
- Bundled turn scripts listed there are `claude-code-turns` and `codex-turns`.
  `scripts/README.md:70-81`.
- Bundled transcript locators listed there are
  `claude-code-locate-transcript` and `codex-locate-transcript`.
  `scripts/README.md:139-145`.
- Quota scripts documented there are `anthropic-usage`, `chatgpt-usage`, and
  `zai-usage`. `scripts/README.md:196-242`.
- This file is not an AC-1 release asset; it is only a candidate
  cross-reference target for AC-3 if Phase 3 chooses to use it.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`
  and `:91-93`.

## 2. Adjacent surfaces inside the blast radius (read-only — contract-bound, not edited)

### WU-13-01 bundle-upload contract

- The bundle-upload contract is contract-bound and read-only for WU-16-01:
  ticket AC-6 says Linux `.deb`, macOS `.dmg`, Windows `.msi/.exe`, and the
  three suffixed bare binaries must continue to publish.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:77-79`.
- Ticket anti-scope says not to change the binary upload step's
  platform-suffix naming. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:123-126`.
- Current build matrix rows and bundle names are asserted by
  `src-tauri/tests/release_yml_contract.rs:14-46`, `:77-103`, `:124-150`,
  and `:171-206`.
- Current bare-binary target suffix behavior is asserted by
  `src-tauri/tests/release_yml_contract.rs:48-76`, `:105-123`, `:152-170`,
  and `:264-273`.
- Prior WU-13 terrain to reuse, not copy: the current release flow was mapped
  in `research/13-release-restore-problem-map.md:211-220`; WU-13 hookpoints
  record the collect/upload/download/release-file invariants in
  `research/13-release-restore-hookpoints.md:455-551`; the proposal and risk
  gate preserve the same bare-binary/bundle split in
  `proposals/13-release-restore.md:256-262` and
  `risk/13-release-restore-supported-surface.md:110-129`.

### Seven adapter scripts

- Adapter scripts themselves are read-only here. Ticket anti-scope states not
  to modify scripts because they are correct as shipped in WU-15-01.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:130-133`.
- Exact AC-1 script set from the ticket:
  `claude-code-turns`, `codex-turns`, `anthropic-usage`, `chatgpt-usage`,
  `zai-usage`, `claude-code-locate-transcript`, and
  `codex-locate-transcript`.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`.
- Existing `scripts/claude-code-turns` is a Python reference adapter for
  Claude Code session turns. `scripts/claude-code-turns:1-4`.
- Existing `scripts/codex-turns` is a Python reference adapter for Codex CLI
  session turns. `scripts/codex-turns:1-5`.
- Existing `scripts/anthropic-usage` is a Bash quota script for Claude /
  Anthropic OAuth usage. `scripts/anthropic-usage:1-5`.
- Existing `scripts/chatgpt-usage` is a Bash quota script for ChatGPT /
  OpenAI OAuth usage. `scripts/chatgpt-usage:1-5`.
- Existing `scripts/zai-usage` is a Bash quota script for Z.ai usage.
  `scripts/zai-usage:1-5`.
- Existing `scripts/claude-code-locate-transcript` is a Bash wrapper around
  embedded Python locator logic. `scripts/claude-code-locate-transcript:1-5`.
- Existing `scripts/codex-locate-transcript` is a Bash wrapper around embedded
  Python locator logic. `scripts/codex-locate-transcript:1-5`.

### `src-tauri` build artifacts flowing into `artifacts/`

- Tauri build output flows into `artifacts/` only through the platform collect
  steps; WU-16-01 does not need to change Tauri-side packaging internals.
  `.github/workflows/release.yml:137-162`.
- Linux path: `.deb` from
  `src-tauri/target/${{ matrix.target }}/release/bundle/deb/*.deb` and bare
  binary from `src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner`.
  `.github/workflows/release.yml:139-145`.
- macOS path: `.dmg` from
  `src-tauri/target/${{ matrix.target }}/release/bundle/dmg/*.dmg` and bare
  binary from `src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner`.
  `.github/workflows/release.yml:145-150`.
- Windows path: `.msi`, NSIS `.exe`, and bare
  `oulipoly-agent-runner.exe` from the Windows target directory.
  `.github/workflows/release.yml:151-158`.
- Ticket out-of-scope explicitly excludes bundling scripts into `.deb` /
  `.dmg` / `.msi`, embedding scripts in the binary, automatic PATH install,
  non-`scripts/` release content, and routing/quota/migration/body-storage code
  paths. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:95-107`.

## 3. Out-of-scope but commonly-confused surfaces (explicitly excluded)

- `scripts/migrate-model-names.sh` is a one-time model TOML rename helper, not
  an adapter. Its own comment says it renames model TOML files to use the
  `~` separator. `scripts/migrate-model-names.sh:1-5`.
- `scripts/migrate-model-names.sh` is excluded by AC-1 because AC-1 lists only
  the seven adapter filenames and excludes non-adapter entries.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`.
- `scripts/tests/` is a fixture/test directory for scripts, not a release asset
  target. The repository currently contains `scripts/tests/chatgpt-usage.test.sh`
  and fixture files under `scripts/tests/fixtures/chatgpt-usage/`.
  `scripts/tests/chatgpt-usage.test.sh:1-8`.
- The `scripts/tests/` contents are excluded per AC-1's explicit exclusion of
  `scripts/tests/`. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-44`.
- `scripts/README.md` is adapter contract documentation. It is excluded as a
  release asset by AC-1, while remaining optional as a cross-reference doc edit
  point under Code Boundary. `scripts/README.md:1-5` and
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-44`,
  `:91-93`.
- `.deb` / `.dmg` / `.msi` packaging logic, including Tauri-side bundle wiring,
  is out of scope. The ticket defers bundling scripts into those packages
  because it would require Tauri config changes and per-platform install-path
  decisions. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:95-99`.
- Rust runtime body-ingestion paths are out of scope here. The ticket source
  identifies this WU as closing the install-process gap after WU-15-01 fixed
  body persistence in the binary. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:8-22`
  and `:160-167`.
- Runtime version-skew detection logic is out of scope. Ticket anti-scope says
  not to add runner logic that refuses to ingest when scripts are too old and
  not to add stale-script compatibility shims.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:123-131`.
- Frontend code under `src/` is out of scope. Ticket out-of-scope excludes
  routing/quota/migration/body-storage code paths and names only release YAML,
  the release contract test, README, and optional `scripts/README.md` as
  in-scope files. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:81-93`
  and `:95-107`.
- Prior-WU tests under `tests/routing_fanout_rca/`,
  `tests/session_migration_rca/`, `tests/empty_bodies_ref_rca/`,
  `tests/session_lock_cross_platform.rs`, and `tests/initiative_06_*` are out
  of WU-16-01's test boundary. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:108-121`.

## 4. Current-state risks already living on this surface

- RC-1 root cause: `release.yml` does not include adapter scripts in the
  publish step, so users installing from release-tag binaries get a binary that
  expects body-aware scripts but receive no scripts. The current publish step
  lists only `files: artifacts/*`. `.github/workflows/release.yml:177-181`.
- RC-1 user symptom: `session_turns.body` stays `NULL` until the user manually
  updates scripts from a repo clone. The ticket records this symptom at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:24-30`.
- RC-1 package audit evidence: the v0.1.26 `.deb` audited by WU-15-01 did not
  bundle adapter scripts. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:19-21`.
- WU-13-01 contract fragility: `src-tauri/tests/release_yml_contract.rs`
  structurally matches the `softprops/action-gh-release@v2` publish step and
  currently asserts `with.files == "artifacts/*"`.
  `src-tauri/tests/release_yml_contract.rs:253-262`.
- WU-13-01 non-regression risk: adding script upload coverage must be additive
  to the existing bare-binary platform-suffix assertions and bundle assertions,
  because AC-6 requires those release paths to keep publishing.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:77-79`
  and `src-tauri/tests/release_yml_contract.rs:14-273`.
- Release-flow fragility: artifacts visible to the publish step come from two
  separate sources today: repository checkout in the release job and merged
  matrix artifacts downloaded into `artifacts/`. `.github/workflows/release.yml:168-181`.
- Silent version skew: binary and scripts can diverge if a release is cut
  without making scripts release assets, which is why AC-3 requires a README
  note that matched script and binary versions are needed for body ingestion.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:60-68`.
- Documentation gap: README's existing adapter install snippet is a
  source-build / repo-checkout shape (`install -m 755 scripts/... ~/.local/bin/`)
  and does not currently tell binary-install users to download scripts from the
  matching release tag. `README.md:340-350`.
- README adjacency risk: the current install snippet under "Reference quota
  adapters" installs quota and turn scripts but not the transcript locator
  scripts that AC-1 lists as release assets. `README.md:340-350` and
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`.

## 5. Adjacent supported paths that exercise the surface today

- Release CI flow: manual `workflow_dispatch` runs lint, test, version, build,
  and release jobs; the publish path is version/tag resolution, matrix build,
  artifact upload, artifact download, tag creation, and
  `softprops/action-gh-release@v2`. `.github/workflows/release.yml:3-181`.
- The release job creates the tag only after build artifacts exist, then
  publishes generated release notes and files. `.github/workflows/release.yml:173-181`.
- This is the single supported path for end-user release binaries in the mapped
  surface; the ticket says scripts currently ship only through a repo clone and
  AC-1 targets release assets. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:34-38`
  and `:42-53`.
- User-install path today: README documents a manual local build binary install
  at `README.md:46-51` and a repo-checkout adapter script install snippet at
  `README.md:340-350`.
- User-install path after the planned surface change must remain tied to README
  §"Reference quota adapters" plus a release-asset `gh release download`
  snippet, per AC-3 and ticket notes. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:60-68`
  and `:143-154`.
- Structural-contract path: `src-tauri/tests/release_yml_contract.rs` parses
  `.github/workflows/release.yml` and gates merge-time workflow-shape
  regressions through Rust tests. `src-tauri/tests/release_yml_contract.rs:6-18`
  and `:228-262`.
- Prior-WU release-flow artifacts are still useful as terrain references only:
  `research/13-release-restore-problem-map.md:211-220`,
  `research/13-release-restore-hookpoints.md:455-551`,
  `proposals/13-release-restore.md:582-616`, and
  `risk/13-release-restore-supported-surface.md:110-129`.

## 6. Assumption register (draft — to be carried into Phase 3)

- A1: `softprops/action-gh-release@v2` accepts a multi-line glob and/or list in
  its `files:` input. Current local evidence is the existing single glob
  `files: artifacts/*` at `.github/workflows/release.yml:177-181`; upstream
  documentation says `with.files` is a newline-delimited list of glob
  expressions and may also list files by name directly. Source:
  https://github.com/softprops/action-gh-release lines 359-405 and 456-464
  as observed on 2026-05-04. Cheap falsification: Phase 3 can inspect the
  action README/action metadata for v2 specifically; Phase 4 can reject if the
  v2 input differs; Phase 5 can test parse shape against the exact workflow
  YAML.
- A2: Adding script paths to the publish step preserves filenames verbatim
  enough for the AC-1 asset names, matching the current bare-binary upload
  behavior from WU-13-01. Current local evidence is that staged bare binaries
  are copied into `artifacts/oulipoly-agent-runner-${{ matrix.target }}` and
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe`, then uploaded via
  `files: artifacts/*`. `.github/workflows/release.yml:139-181`.
  Cheap falsification: Phase 3 can check `softprops` filename handling and
  GitHub release-asset naming docs; Phase 4 can flag mismatch with AC-1's
  no-extension-change requirement; Phase 5 can prefer the insertion path whose
  asset names are structurally testable before any trial release.
- A3: All seven AC-1 scripts live under `scripts/` at HEAD and have stable
  names. Current evidence: the ticket lists the seven names at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`,
  and each corresponding file has executable script content at
  `scripts/claude-code-turns:1-4`, `scripts/codex-turns:1-5`,
  `scripts/anthropic-usage:1-5`, `scripts/chatgpt-usage:1-5`,
  `scripts/zai-usage:1-5`, `scripts/claude-code-locate-transcript:1-5`, and
  `scripts/codex-locate-transcript:1-5`. Cheap falsification: Phase 3 can run
  `test -f` for the seven paths; Phase 4 can reject any rename drift; Phase 5
  can bind the final list from the same paths before implementation.
- A4: The structural release-yml test parses YAML the same way it did in
  WU-13-01; extending it does not require a rewrite. Current evidence:
  `src-tauri/tests/release_yml_contract.rs` reads the workflow, parses with
  `serde_yml::from_str`, and navigates strings/bools/sequences through helper
  functions. `src-tauri/tests/release_yml_contract.rs:8-18` and `:334-377`.
  Cheap falsification: Phase 3 can sketch the added assertion against the
  current helper set; Phase 4 can flag if block scalar parsing changes the
  expected `Value::String` shape; Phase 5 can verify the exact assertion
  extension point compiles mentally before Phase 6b edits.
- A5: README has an existing §"Reference quota adapters" install snippet, or a
  clearly equivalent install section, where the `gh release download` snippet
  can be appended adjacent to current source-build instructions. Current
  evidence: `README.md:332-350` contains the subsection and source-build
  install snippet; `README.md:352-410` is the neighboring session-ingestion and
  locator documentation. Cheap falsification: Phase 3 can decide that a
  different README location is more accurate; Phase 4 can reject if the chosen
  location obscures the source-build path AC-3 says must remain valid; Phase 5
  can bind exact insertion lines before doc edits.
- A6: `scripts/README.md` exists and can host a one-line cross-reference to the
  new release-asset install path if Phase 3 uses the optional doc point.
  Current evidence: `scripts/README.md:1-5` documents adapter scripts, and the
  ticket marks the file optional in Code Boundary at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:91-93`.
  Cheap falsification: Phase 3 can decide the optional edit has no value;
  Phase 4 can reject if it turns into a second install procedure; Phase 5 can
  verify whether a one-line cross-reference has a stable local anchor.

## 7. Termination signals to watch in Phase 4

- Invalidated-assumption signal: any of A1 through A6 fails in a way that makes
  the ticket's Code Boundary or Test Boundary insufficient. See assumption
  register above.
- Non-positive value signal: an already-supported release channel is discovered
  that publishes all seven adapter scripts with matched binary versions. The
  ticket evidence makes this unlikely because it says scripts ship only through
  a repo clone and the audited `.deb` lacks scripts.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:19-21`
  and `:34-38`.
- Surface-widening signal: the publish job's `files:` shape cannot accept
  script paths without changing artifact aggregation logic beyond the release
  job and structural test. Current state is a single `files: artifacts/*`
  scalar at `.github/workflows/release.yml:177-181`.
- Surface-widening signal: making scripts available as release assets would
  require Tauri bundle configuration or package install-path decisions. Ticket
  out-of-scope explicitly defers `.deb` / `.dmg` / `.msi` script bundling.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:95-99`.
- Surface-widening signal: AC-2 cannot be expressed as an additive extension to
  `src-tauri/tests/release_yml_contract.rs` and instead requires replacing the
  WU-13 bare-binary assertions. Current test structure already exposes the
  `gh_release` publish step at `src-tauri/tests/release_yml_contract.rs:253-262`.
- Scope-confusion signal: implementation tries to include
  `scripts/migrate-model-names.sh`, `scripts/tests/`, or `scripts/README.md`
  as release assets despite AC-1's exact script list and exclusions.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:42-53`.
- Scope-confusion signal: implementation adds runtime stale-script detection,
  compatibility shims, frontend changes, or body-ingestion changes. Ticket
  anti-scope and out-of-scope exclude these paths at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:95-107`
  and `:123-133`.

## 8. Open questions for Phase 5 / Phase 3

- Should scripts upload as individual files or as a `scripts.tar.gz` bundle?
  The ticket recommends individual files because they preserve direct
  `gh release download --pattern <name>` behavior and match the WU-13
  bare-binary release-asset pattern; `scripts.tar.gz` would reduce asset count
  but make single-script install harder. This phase does not bind the answer.
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:135-142`.
- Where in the publish job is the cleanest insertion point: extending the
  existing `files: artifacts/*` surface by making scripts visible under
  `artifacts/`, or appending explicit `scripts/<name>` paths to the
  `softprops/action-gh-release@v2` `files:` input? Current state supports both
  as adjacent surfaces because the release job checks out the repo and also
  downloads artifacts. `.github/workflows/release.yml:168-181`. Phase 5 must
  answer this; Phase 2.5 only records the terrain.
- Whether `scripts/README.md` should host a one-line cross-reference. The
  ticket marks it optional, and the file currently documents adapter contracts
  rather than end-user release installation. `scripts/README.md:1-5` and
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:91-93`.
  Phase 3 decides whether the optional doc edit has net clarity.
- Should AC-2 structural coverage be kept inside the existing
  `release_yml_restores_windows_and_target_suffixed_bare_binaries` test or
  split into a second test in the same file? Existing helper functions already
  support both shapes. `src-tauri/tests/release_yml_contract.rs:6-18`,
  `:253-262`, and `:320-377`. Phase 5 can choose the least noisy test shape.

Status: ready for Phase 3
