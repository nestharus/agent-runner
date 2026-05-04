# WU-16-01 Release Scripts Hookpoints

Phase: 5 hookpoint research

Required inputs read: `research/16-release-scripts-problem-map.md`,
`proposals/16-release-scripts.md`, all four Phase 4 reports under
`risk/16-release-scripts-*.md`, `.github/workflows/release.yml`,
`src-tauri/tests/release_yml_contract.rs`, `scripts/`, `scripts/README.md`,
`README.md`, WU-13-01 precedent paths
`research/13-release-restore-hookpoints.md`,
`proposals/13-release-restore.md`,
`risk/13-release-restore-supported-surface.md`, and WU-15-01 evidence paths
`research/15-empty-bodies-ref-problem-map.md`,
`proposals/15-empty-bodies-ref.md`.

## 1. Reuse points

1. Release job checkout.

   The publish job already checks out the repo before release upload.
   `.github/workflows/release.yml:164-181`.

   Reuse: list `scripts/<name>` directly in `softprops/action-gh-release@v2`
   `with.files`. The checkout at `.github/workflows/release.yml:168` is the
   source-of-truth that makes repo scripts visible in the publish job.

2. Release job artifact download.

   The publish job already downloads matrix artifacts into `artifacts/` with
   `merge-multiple: true`, then publishes `artifacts/*`.
   `.github/workflows/release.yml:169-181`.

   Reuse: keep `artifacts/*` as a release-file entry so WU-13-01 bundles and
   bare binaries keep publishing through the existing path.

3. Existing release contract test.

   `release_yml_restores_windows_and_target_suffixed_bare_binaries` already
   reads `../.github/workflows/release.yml`, parses it with `serde_yml`, and
   asserts matrix, bundle, artifact upload/download, release upload, and
   bare-binary contracts. `src-tauri/tests/release_yml_contract.rs:6-274`.

   Reuse: extend this test in place. Do not create a new parser or duplicate
   the workflow fixture.

4. Existing YAML helpers.

   Reuse `step_by_uses(steps, uses)` at
   `src-tauri/tests/release_yml_contract.rs:327-332` and `string_at(root,
   label, path)` at `src-tauri/tests/release_yml_contract.rs:341-346`.

   Exact Phase 6b lookup pattern:

   ```rust
   let gh_release = step_by_uses(release_steps, "softprops/action-gh-release@v2");
   let files = string_at(
       gh_release,
       "jobs.release.steps[softprops/action-gh-release@v2].with.files",
       &["with", "files"],
   );
   ```

   This extends the existing publish-step lookup/assertion at
   `src-tauri/tests/release_yml_contract.rs:253-262`.

5. Existing `BTreeSet` style.

   The test already imports `BTreeSet` and uses set equality for
   order-independent workflow assertions.
   `src-tauri/tests/release_yml_contract.rs:1-2`,
   `src-tauri/tests/release_yml_contract.rs:25-46`,
   `src-tauri/tests/release_yml_contract.rs:264-273`.

   Reuse: collect trimmed non-empty `with.files` lines into `BTreeSet<String>`
   and compare against the exact eight-entry set.

6. README install anchor.

   `README.md` already has `**Reference quota adapters** (in
   [scripts/](scripts/)):` at `README.md:332`, and the current source-build
   install snippet spans `README.md:340-350`.

   Reuse: keep this as the canonical install-doc surface. Insert the binary
   release-asset snippet after line 350 and before `## Session Ingestion` at
   `README.md:352`.

7. Existing source-build snippet.

   Preserve this exact existing form:

   ```bash
   install -m 755 \
     scripts/anthropic-usage \
     scripts/chatgpt-usage \
     scripts/zai-usage \
     scripts/claude-code-turns \
     scripts/codex-turns \
     ~/.local/bin/
   ```

   Source: `README.md:340-350`.

8. `scripts/README.md` opening anchor.

   `scripts/README.md` opens by defining adapter scripts as standalone TOML-wired
   executables, not binary-linked code. `scripts/README.md:1-5`.

   Reuse: add one cross-reference sentence near the opening only.

9. Seven existing AC-1 scripts.

   The release assets already exist as exact files:

   - `scripts/claude-code-turns:1-6`
   - `scripts/codex-turns:1-6`
   - `scripts/anthropic-usage:1-6`
   - `scripts/chatgpt-usage:1-6`
   - `scripts/zai-usage:1-6`
   - `scripts/claude-code-locate-transcript:1-6`
   - `scripts/codex-locate-transcript:1-6`

   Reuse: upload these files as release assets. Do not edit script bodies.

## 2. Extension points

1. `.github/workflows/release.yml:177-181`.

   Current surface: `softprops/action-gh-release@v2` sets `tag_name`,
   `generate_release_notes: true`, and `files: artifacts/*`.
   `.github/workflows/release.yml:177-181`.

   Phase 6 edit:

   - Convert `files: artifacts/*` to a YAML block scalar.
   - Keep `artifacts/*`.
   - Add exactly:
     `scripts/claude-code-turns`,
     `scripts/codex-turns`,
     `scripts/anthropic-usage`,
     `scripts/chatgpt-usage`,
     `scripts/zai-usage`,
     `scripts/claude-code-locate-transcript`,
     `scripts/codex-locate-transcript`.

   Compatibility anchor: `actions/checkout@v4` at
   `.github/workflows/release.yml:168` makes these paths available directly.

2. `src-tauri/tests/release_yml_contract.rs:253-262`.

   Current surface: a single-value assertion that
   `with.files == "artifacts/*"`.
   `src-tauri/tests/release_yml_contract.rs:253-262`.

   Phase 6 edit:

   - Keep `let gh_release = step_by_uses(release_steps, "softprops/action-gh-release@v2");`.
   - Replace scalar equality with exact set equality:

   ```rust
   let files = string_at(
       gh_release,
       "jobs.release.steps[softprops/action-gh-release@v2].with.files",
       &["with", "files"],
   );
   let actual_files = files
       .lines()
       .map(str::trim)
       .filter(|line| !line.is_empty())
       .map(str::to_string)
       .collect::<BTreeSet<_>>();
   ```

   - Compare to `BTreeSet::from([...])` containing `artifacts/*` plus the seven
     explicit script paths.

   Helper citations:
   `step_by_uses` at `src-tauri/tests/release_yml_contract.rs:327-332`;
   `string_at` at `src-tauri/tests/release_yml_contract.rs:341-346`;
   `BTreeSet` import at `src-tauri/tests/release_yml_contract.rs:2`.

3. `README.md:350-352`.

   Insertion point:

   - Existing source-build snippet ends at `README.md:350`.
   - `## Session Ingestion` starts at `README.md:352`.

   Phase 6 edit:

   - Insert a binary-install paragraph and command block between those lines.
   - Preserve `README.md:340-350`.
   - State that binary-install users should install scripts from the same
     release tag as the binary.
   - State that mismatched stale scripts may silently omit `body`, leaving new
     ingests with empty `session_turns.body`.
   - Use `gh release download v0.1.X --repo nestharus/agent-runner`.
   - Include one `--pattern` per seven script basename and `chmod +x` for all
     seven installed files.

   Proposal source: `proposals/16-release-scripts.md:166-189`,
   `proposals/16-release-scripts.md:569-579`.

4. `scripts/README.md:1-5`.

   Phase 6 edit: add one sentence after the opening paragraph:
   `For release-asset installation of the bundled reference adapters, see README §Reference quota adapters.`
   Do not add command blocks or parallel install instructions.

   Scope source:
   `research/16-release-scripts-problem-map.md:170-189`,
   `proposals/16-release-scripts.md:581-587`,
   `risk/16-release-scripts-shortcut.md:160-189`.

## 3. Conflicting systems

1. WU-13-01 bare-binary suffix contract.

   Current collect steps suffix bare binaries by target:
   `.github/workflows/release.yml:139-145`,
   `.github/workflows/release.yml:145-150`,
   `.github/workflows/release.yml:151-158`.

   Current assertions preserve this behavior:
   `src-tauri/tests/release_yml_contract.rs:48-76`,
   `src-tauri/tests/release_yml_contract.rs:105-123`,
   `src-tauri/tests/release_yml_contract.rs:152-170`,
   `src-tauri/tests/release_yml_contract.rs:264-273`.

   Constraint: do not rename bare binaries, remove target suffixes, alter the
   Windows `.exe` suffix, or weaken these assertions.

2. WU-13-01 bundle upload contract.

   Bundle staging is unchanged:
   `.github/workflows/release.yml:139-158`.

   Bundle assertions are unchanged:
   `src-tauri/tests/release_yml_contract.rs:77-103`,
   `src-tauri/tests/release_yml_contract.rs:124-150`,
   `src-tauri/tests/release_yml_contract.rs:171-206`.

   Constraint: do not touch platform collect steps, Tauri bundle internals, or
   bundle assertions.

3. Existing artifact upload/download wiring.

   Build jobs upload `artifacts/*` with matrix target names.
   `.github/workflows/release.yml:159-162`.

   Release job downloads merged artifacts to `artifacts`.
   `.github/workflows/release.yml:169-172`.

   Current test assertions cover this wiring at
   `src-tauri/tests/release_yml_contract.rs:208-251`.

   Constraint: do not use Option B. Do not copy repo scripts into `artifacts/`;
   append direct `scripts/<name>` entries to softprops `files`.

4. Softprops file-list and asset-name behavior.

   Softprops v2 README documents `files` as newline-delimited globs and allows
   direct file names. It shows a YAML block scalar for multiple files.
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/README.md:80-140`.

   Softprops v2 metadata defines `files` as newline-delimited path globs.
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/action.yml:27-29`.

   Softprops v2 source splits `INPUT_FILES` by newline, trims through
   `smartSplit`, and filters empty patterns.
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/util.ts:55-87`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/util.ts:97-107`.

   Softprops v2 uploads each matched file individually and derives asset names
   from `basename(path)`.
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/main.ts:56-80`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:257-263`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:304-335`.

   Exception: GitHub may normalize special or non-ASCII asset filenames, and the
   final download name remains GitHub-controlled.
   `https://github.com/softprops/action-gh-release/tree/v2:461-482`.

   Constraint: the AC-1 names are ASCII hyphenated basenames, so this exception
   does not require a different hookpoint.

5. Exact seven-script surface and exclusions.

   Include exactly:
   `scripts/claude-code-turns`,
   `scripts/codex-turns`,
   `scripts/anthropic-usage`,
   `scripts/chatgpt-usage`,
   `scripts/zai-usage`,
   `scripts/claude-code-locate-transcript`,
   `scripts/codex-locate-transcript`.

   Confirmed at:
   `scripts/claude-code-turns:1-6`,
   `scripts/codex-turns:1-6`,
   `scripts/anthropic-usage:1-6`,
   `scripts/chatgpt-usage:1-6`,
   `scripts/zai-usage:1-6`,
   `scripts/claude-code-locate-transcript:1-6`,
   `scripts/codex-locate-transcript:1-6`.

   Exclude:

   - `scripts/README.md`, documentation only. `scripts/README.md:1-5`.
   - `scripts/tests/`, script test/fixture surface.
     `scripts/tests/chatgpt-usage.test.sh:1-10`.
   - `scripts/migrate-model-names.sh`, model TOML migration helper.
     `scripts/migrate-model-names.sh:1-5`.

   Constraint: the structural assertion must use exact set equality to reject
   both omitted scripts and accidental `scripts/*` broadening.

6. WU-15-01 body-aware ingest boundary.

   WU-15-01 made body-aware adapter output relevant to persisted
   `session_turns.body`. Evidence paths:
   `research/15-empty-bodies-ref-problem-map.md`,
   `proposals/15-empty-bodies-ref.md`.

   Constraint: WU-16-01 closes release/install delivery only. Do not change
   runtime ingest logic, body schema, migrations, routing, quota logic, or UI.

7. README source-build asymmetry.

   The existing source-build snippet installs five scripts and omits both
   transcript locators. `README.md:340-350`.

   Risk SS-01 records this as non-blocking because AC-3 only requires that the
   source-build snippet remain valid, while the new binary-install snippet must
   include all seven. `risk/16-release-scripts-supported-surface.md:96-122`.

   Constraint: do not drop any of the seven from the new binary-install snippet.
   Preserve the existing source-build snippet unless Phase 6 explicitly records
   a deliberate doc expansion.

8. Live-release evidence boundary.

   Structural YAML tests do not prove live GitHub release-page materialization.
   This residual is already called out at
   `proposals/16-release-scripts.md:363-366`,
   `proposals/16-release-scripts.md:395-397`,
   `risk/16-release-scripts-shortcut.md:58-86`,
   `risk/16-release-scripts-supported-surface.md:150-170`.

   Constraint: if Phase 6b runs no trial release, write
   `risk/16-release-scripts-test-residuals.md`.

## 4. Deletion candidates

None.

This WU is additive: add seven direct script paths to the existing release
upload list, replace one scalar assertion with an exact set assertion, add one
README binary-install snippet, and add one optional `scripts/README.md`
cross-reference sentence. Do not delete `artifacts/*` from
`.github/workflows/release.yml:181`, WU-13 assertions in
`src-tauri/tests/release_yml_contract.rs:14-273`, the existing source-build
snippet at `README.md:340-350`, or any of the seven adapter scripts.

## 5. Open-question resolutions (Q-A through Q-H)

Q-A. Confirmed: `softprops/action-gh-release@v2` accepts a multi-line
`files:` input where each non-empty trimmed line is a glob/path. A1 is locked.

Evidence:

- v2 README documents newline-delimited globs and direct file names.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/README.md:80-88`.
- v2 README shows `files: |` with multiple entries.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/README.md:114-140`.
- v2 action metadata defines `files` as newline-delimited path globs.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/action.yml:27-29`.
- v2 source splits by newline, trims entries, and filters empty patterns.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/util.ts:55-87`,
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/util.ts:97-107`.
- Local workflow already uses `files: artifacts/*`.
  `.github/workflows/release.yml:177-181`.

Conclusion: a block scalar with `artifacts/*` plus seven `scripts/<name>`
entries is compatible.

Q-B. Confirmed: adding seven `scripts/<name>` paths preserves filenames as
individual release asset basenames, with no extension munging and no `.zip`
rollup. A2 is locked.

Evidence:

- v2 source expands `input_files` and uploads each matched file individually.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/main.ts:56-80`.
- v2 source derives asset metadata with `name: basename(path)`.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:257-263`.
- v2 source appends that name to the upload endpoint.
  `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:304-335`.
- WU-13-01 local behavior uses the same as-named release-asset path for
  target-suffixed bare binaries staged under `artifacts/`.
  `.github/workflows/release.yml:139-181`; precedent paths:
  `research/13-release-restore-hookpoints.md`,
  `proposals/13-release-restore.md`,
  `risk/13-release-restore-supported-surface.md`.

Softprops-specific exception: GitHub may normalize special or non-ASCII raw
asset filenames, and the final download name remains GitHub-controlled.
`https://github.com/softprops/action-gh-release/tree/v2:461-482`.

Conclusion: the seven ASCII hyphenated script basenames are compatible with
direct `scripts/<name>` upload.

Q-C. Confirmed: all seven AC-1 scripts exist at HEAD. A3 is locked.

Confirmed files:

- `scripts/claude-code-turns:1-6`
- `scripts/codex-turns:1-6`
- `scripts/anthropic-usage:1-6`
- `scripts/chatgpt-usage:1-6`
- `scripts/zai-usage:1-6`
- `scripts/claude-code-locate-transcript:1-6`
- `scripts/codex-locate-transcript:1-6`

Confirmed exclusions:

- `scripts/README.md:1-5`
- `scripts/tests/chatgpt-usage.test.sh:1-10`
- `scripts/migrate-model-names.sh:1-5`

Conclusion: hardcode the exact seven-path set; do not use discovery or
`scripts/*`.

Q-D. Confirmed: `release_yml_contract.rs` parses YAML the same way it did in
WU-13-01, and extending the publish assertion is straightforward. A4 is locked.

Evidence:

- The test reads and parses the workflow with `serde_yml::from_str`.
  `src-tauri/tests/release_yml_contract.rs:8-12`.
- The publish step is found with `step_by_uses`.
  `src-tauri/tests/release_yml_contract.rs:253`,
  `src-tauri/tests/release_yml_contract.rs:327-332`.
- `with.files` is read as `Value::String` via `string_at`.
  `src-tauri/tests/release_yml_contract.rs:254-262`,
  `src-tauri/tests/release_yml_contract.rs:341-346`.

Exact Phase 6b pattern:

```rust
let gh_release = step_by_uses(release_steps, "softprops/action-gh-release@v2");
let files = string_at(
    gh_release,
    "jobs.release.steps[softprops/action-gh-release@v2].with.files",
    &["with", "files"],
);
let actual_files = files
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
```

Conclusion: extend the existing test in place with set equality over
`actual_files`.

Q-E. Confirmed: README has a stable `Reference quota adapters` insertion
point. A5 is locked.

Evidence:

- Heading/table anchor: `README.md:332-338`.
- Existing source-build install snippet: `README.md:340-350`.
- Next section starts at `README.md:352`.

Insertion point: add the `gh release download` snippet after `README.md:350`
and before `README.md:352`, preserving `README.md:340-350`.

Q-F. Confirmed: `scripts/README.md` exists and has a one-line
cross-reference target. A6 is locked.

Evidence:

- Opening adapter-contract paragraph: `scripts/README.md:1-5`.
- Proposal's one-line target: `proposals/16-release-scripts.md:581-587`.
- Drift warning against duplicate install procedures:
  `risk/16-release-scripts-shortcut.md:160-189`.

Resolution: include the optional one-line cross-reference after
`scripts/README.md:3-5`; do not add commands there.

Q-G. Resolved: choose Option A, not Option B.

Evidence:

- Proposal binds Option A.
  `proposals/16-release-scripts.md:515-528`.
- Checkout makes direct `scripts/<name>` paths visible.
  `.github/workflows/release.yml:168`.
- `artifacts/` is already the build-output aggregation path.
  `.github/workflows/release.yml:139-172`.
- Phase 4 agrees Option A avoids staging non-build repo files into
  `artifacts/`. `risk/16-release-scripts-scope.md:36-59`,
  `risk/16-release-scripts-supported-surface.md:226-232`.

Q-H. Resolved: keep structural coverage inside the existing test.

Evidence:

- Existing test owns publish files at
  `src-tauri/tests/release_yml_contract.rs:253-262`.
- The same test owns WU-13 release non-regression assertions at
  `src-tauri/tests/release_yml_contract.rs:14-273`.
- Helpers are already in the same file.
  `src-tauri/tests/release_yml_contract.rs:327-346`.
- Proposal preference is in-place extension.
  `proposals/16-release-scripts.md:388-394`,
  `proposals/16-release-scripts.md:563-567`.

Conclusion: extend the existing release contract test in place.

## 6. Touched-surface delta vs problem map

1. No invalidation.

   Problem-map A1-A6 and proposal A1-A6 remain valid.
   `research/16-release-scripts-problem-map.md:367-424`,
   `proposals/16-release-scripts.md:221-332`.

2. A1 and A2 are stronger after Phase 5.

   Problem-map draft evidence at
   `research/16-release-scripts-problem-map.md:369-388` is now backed by v2
   README, action metadata, parser, per-file upload, and basename evidence:
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/README.md:80-140`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/action.yml:27-29`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/util.ts:55-87`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/main.ts:56-80`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:257-263`,
   `https://raw.githubusercontent.com/softprops/action-gh-release/v2/src/github.ts:304-335`.

3. A3 is unchanged and confirmed at HEAD.

   Problem-map script list at
   `research/16-release-scripts-problem-map.md:389-398` matches current files:
   `scripts/claude-code-turns:1-6`,
   `scripts/codex-turns:1-6`,
   `scripts/anthropic-usage:1-6`,
   `scripts/chatgpt-usage:1-6`,
   `scripts/zai-usage:1-6`,
   `scripts/claude-code-locate-transcript:1-6`,
   `scripts/codex-locate-transcript:1-6`.

4. A4 is narrowed to exact helper usage.

   Problem-map helper compatibility at
   `research/16-release-scripts-problem-map.md:399-407` is now bound to
   `step_by_uses` plus `string_at`, then `lines() -> trim -> filter non-empty
   -> BTreeSet<String>`. Local hookpoints:
   `src-tauri/tests/release_yml_contract.rs:253-262`,
   `src-tauri/tests/release_yml_contract.rs:327-346`.

5. A5 is unchanged and confirmed at HEAD.

   Problem-map README terrain at
   `research/16-release-scripts-problem-map.md:138-168`,
   `research/16-release-scripts-problem-map.md:408-416` is bound to insertion
   after `README.md:350` and before `README.md:352`.

6. A6 is resolved to include the optional line.

   Problem-map optionality at
   `research/16-release-scripts-problem-map.md:417-424`,
   `research/16-release-scripts-problem-map.md:474-478` is bound to one
   sentence after `scripts/README.md:1-5`, using proposal wording at
   `proposals/16-release-scripts.md:581-587`.

7. Option A vs Option B is resolved.

   Problem-map open terrain at
   `research/16-release-scripts-problem-map.md:467-473` is resolved to direct
   `scripts/<name>` entries because checkout at `.github/workflows/release.yml:168`
   already exposes repo content and `artifacts/` remains build-output staging at
   `.github/workflows/release.yml:139-172`.

8. Structural-test placement is resolved.

   Problem-map open question at
   `research/16-release-scripts-problem-map.md:479-483` is resolved to in-place
   extension at `src-tauri/tests/release_yml_contract.rs:253-262`.

9. No touched-surface expansion.

    The implementation surface remains `.github/workflows/release.yml`,
    `src-tauri/tests/release_yml_contract.rs`, `README.md`, and optional
    `scripts/README.md`, matching `research/16-release-scripts-problem-map.md:19-190`
    and `proposals/16-release-scripts.md:513-587`.

Status: ready for Phase 6
