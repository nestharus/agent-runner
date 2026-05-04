# WU-16-01 PR Supported-Surface Verification

Phase: 8 PR supported-surface verification
Work unit: `release-scripts`
Branch reviewed: `impl/wu-16-01` at `b4bac1c fix(release): ship adapter scripts as release assets`
Diff scope: `git diff main..HEAD` (one code commit; remainder are
research / proposal / risk artifacts).
Inputs cross-checked: `research/16-release-scripts-problem-map.md`,
`proposals/16-release-scripts.md` §2 supported-surface track and §5
net-value statement, `risk/16-release-scripts-supported-surface.md`
(Phase 4 — verdict LOW, termination NONE),
`tmp/scratch/wu-16-01/ticket.md`.

## Termination signal

`NONE`.

The Phase 4 assumption register A1..A6 was re-checked against the
diff and against HEAD. No assumption is invalidated:

- A1 (softprops `with.files` accepts a newline-delimited list).
  Confirmed — the publish step at
  `.github/workflows/release.yml:177-189` now binds
  `files: |` as a YAML block scalar with `artifacts/*` plus seven
  `scripts/<name>` entries; the structural test at
  `src-tauri/tests/release_yml_contract.rs:253-278` parses the same
  scalar via `serde_yml`, splits on `\n`, trims, and compares as
  `BTreeSet<String>`.
- A2 (basename preserved on direct `scripts/<name>` upload).
  Unchanged. The publish step path entries omit any rename or
  staging, matching the bare-binary precedent at
  `.github/workflows/release.yml:139-158`.
- A3 (seven adapter scripts present under `scripts/`). Confirmed —
  `scripts/` lists exactly the seven AC-1 names plus the excluded
  `README.md`, `migrate-model-names.sh`, and `tests/`.
- A4 (structural test extends without rewrite). Confirmed — the
  edit re-uses the existing `step_by_uses` and `string_at` helpers
  and sits inside the original
  `release_yml_restores_windows_and_target_suffixed_bare_binaries`
  test; bare-binary, matrix, and download-artifact assertions are
  intact.
- A5 (README has stable insertion point). Confirmed — the
  binary-install snippet at `README.md:352-363` is appended
  immediately after the source-build snippet at `README.md:340-350`
  and before `## Session Ingestion`. Source-build snippet is
  byte-identical to pre-PR shape.
- A6 (`scripts/README.md` accepts a one-line cross-reference).
  Confirmed — `scripts/README.md:7` adds the proposal's prescribed
  one-sentence pointer; no second install block is introduced.

Net-value re-check on the diff: the WU-15-01 install-QA gap (binary
upgrade lands a body-aware runner, but stale local adapter scripts
silently NULL `session_turns.body`) is reduced because the publish
step now emits the seven scripts as direct release assets, and the
README path tells binary-install users to fetch them from the same
release tag. Cost is bounded to the four edit points the proposal
priced. No assumption-flip and no negative-value drift; termination
remains `NONE`.

## Verdict

`LOW`.

## Findings

### F-01 — Publish step actually changes the supported surface (RC-1 closure)

- Severity: NON-BLOCKING.
- Statement: `.github/workflows/release.yml:181-189` extends
  `softprops/action-gh-release@v2`'s `with.files` from the single
  scalar `artifacts/*` to a block scalar containing `artifacts/*`
  plus the seven AC-1 script paths. This is the supported-surface
  change — when CI runs the workflow on a tag, the next release
  page will list the seven scripts as downloadable assets under
  their basename, which is exactly the behavior the Phase 4 gate's
  RC-1 net-value claim depends on.
- Evidence: publish step at
  `.github/workflows/release.yml:177-189`; AC-1 list at
  `tmp/scratch/wu-16-01/ticket.md:42-53`; matching basename
  assumption A2 at
  `proposals/16-release-scripts.md:236-249` (cross-referenced via
  the Phase 4 risk file at lines 29-36).
- Why this is non-symbolic: the diff is the surface change. There
  is no flag, gating helper, or shim layer between the YAML edit
  and the release-page asset list.

### F-02 — Structural test enforces the new shape exactly (AC-2 / AC-4)

- Severity: NON-BLOCKING.
- Statement: `src-tauri/tests/release_yml_contract.rs:253-278`
  parses `with.files` as a string, splits on newlines, trims,
  filters empties, collects into `BTreeSet<String>`, and asserts
  set equality against `{artifacts/*, scripts/anthropic-usage,
  scripts/chatgpt-usage, scripts/claude-code-locate-transcript,
  scripts/claude-code-turns, scripts/codex-locate-transcript,
  scripts/codex-turns, scripts/zai-usage}`. This is set equality,
  not subset — a future regression that drops an entry, adds an
  unintended entry, or reverts to the scalar `artifacts/*` shape
  fails this test.
- Evidence: parsed assertion shape at
  `src-tauri/tests/release_yml_contract.rs:259-278`; helper imports
  at `src-tauri/tests/release_yml_contract.rs:1-4`; local run
  `cargo test --manifest-path src-tauri/Cargo.toml --test release_yml_contract`
  → `1 passed; 0 failed`.

### F-03 — README binary-install path documented adjacent to source-build (AC-3)

- Severity: NON-BLOCKING.
- Statement: `README.md:352-363` adds the matched-versions warning
  and the `gh release download` snippet exactly where the proposal
  §2 specifies (between the source-build snippet at
  `README.md:340-350` and `## Session Ingestion` at the next
  heading). The matched-versions warning is in-line with AC-3:
  "scripts and binary versions must match for body ingestion to
  work, because stale scripts may silently omit `body` and leave
  new ingests with empty `session_turns.body`."
- Evidence: README diff hunk at lines 352-363; AC-3 at
  `tmp/scratch/wu-16-01/ticket.md:60-68`; proposal §2 user-install
  surface at `proposals/16-release-scripts.md:151-189`; symptom at
  `tmp/scratch/wu-16-01/ticket.md:24-30`.
- Comparable to source-build: the source-build snippet still
  installs the historical five scripts (no transcript locators).
  This residual is documented at SS-01 in the Phase 4 file
  (`risk/16-release-scripts-supported-surface.md:96-122`) and was
  cleared as a documentation delta rather than a Phase 4
  termination. The PR does not regress that decision: source-build
  callers still get a valid snippet; binary-install callers get
  the seven-script snippet.

### F-04 — Adjacent supported paths unchanged

- Severity: NON-BLOCKING.
- Statement: WU-13-01 bare-binary platform suffixes
  (`oulipoly-agent-runner-${{ matrix.target }}` / `.exe`),
  `actions/upload-artifact@v4` / `actions/download-artifact@v4`
  steps, the `artifacts/*` upload, the matrix entries, the Tauri
  bundle outputs (`.deb` / `.dmg` / `.msi` / NSIS `.exe`), and the
  source-build install snippet are all unmodified. The structural
  test still asserts the matrix at
  `src-tauri/tests/release_yml_contract.rs:14-46`, the per-platform
  collect steps at lines 48-170, the download-artifact step at
  lines 224-251, and the bare-binary suffix hits at lines 280-289.
- Evidence: full diff scope is `.github/workflows/release.yml` (10
  lines: scalar→block-scalar + seven entries), `README.md` (13
  lines: matched-versions paragraph + snippet), `scripts/README.md`
  (2 lines: one-sentence pointer), and
  `src-tauri/tests/release_yml_contract.rs` (30 lines: split-trim
  collect into `BTreeSet<String>` plus seven entries). No other
  code is touched.

### F-05 — Migration / rollback / observability path

- Severity: NON-BLOCKING.
- Migration: none required. The change is additive on release
  assets and on README. Legacy `install -m 755 scripts/*
  ~/.local/bin/` source-build users remain on a still-valid path.
- Rollback: revert the single review commit `b4bac1c` (no
  follow-up commits in the branch). There is no DB migration, no
  schema column, no installed package state, and no released-asset
  state on `main` to unwind beyond the next tag's asset list.
- Observability: visible deployment signal is the GitHub release
  page asset list; merge-time signal is the structural test
  failing if any of the eight required `with.files` entries
  disappear or an unexpected entry appears.
- Evidence: rollback path at
  `proposals/16-release-scripts.md:198-206`; observability stance
  at `proposals/16-release-scripts.md:208-219`; structural test
  ran green locally as recorded in F-02.

### F-06 — Live-release materialization remains a residual (SS-03)

- Severity: NON-BLOCKING.
- Statement: SS-03 in the Phase 4 file flagged that AC-2's
  "structural test plus trial release" pair has a live-vs-shape
  split. The PR does the structural side; whether a trial release
  is run is a Phase 6b decision and, if not run, must be recorded
  in `risk/16-release-scripts-test-residuals.md`. That residual
  artifact already exists in this branch
  (`risk/16-release-scripts-test-residuals.md`, 129 lines) per the
  diff stat, so the residual closure channel is in place.
- Evidence: residual artifact exists in the branch (diff stat);
  Phase 4 SS-03 closure expectation at
  `risk/16-release-scripts-supported-surface.md:150-170`.

## Observations

- The PR is a single commit (`b4bac1c`) that touches exactly the
  four files the proposal pre-budgeted: workflow, structural test,
  README, and `scripts/README.md`. No collateral edits.
- The structural test is a real guard, not a placeholder. It
  parses the YAML body, splits the block scalar, and compares as
  a `BTreeSet`, so future drift in either direction (drop / add /
  rename) breaks the gate.
- The user-install README snippet matches the ticket's suggested
  shape verbatim (seven `--pattern` flags, `--dir ~/.local/bin/`,
  bash brace-expansion `chmod +x`), preserving the version
  placeholder `v0.1.X` for users to substitute.
- The matched-versions warning is positioned where users actually
  read it: directly before the `gh release download` snippet,
  inside `§Reference quota adapters`, with the symptom phrased in
  the same terms as the WU-15-01 ticket
  (`session_turns.body` empty until manual update).
- The diff does not introduce a runtime version-skew check, a
  scripts.tar.gz bundle, or a Tauri-bundle script embedding —
  consistent with anti-scope at
  `proposals/16-release-scripts.md:14-71` and ticket anti-scope at
  `tmp/scratch/wu-16-01/ticket.md:123-133`.
- Round-2 audit closures (AUDIT-01 set-literal cleanup, AUDIT-02
  residual-risk artifact paths) remain consistent with the PR; the
  residual artifact `risk/16-release-scripts-test-residuals.md`
  exists in the branch.
- No symbolic hardening: there is no defensive guard, no fallback
  branch, and no flag wrapping the new script entries — the
  `files:` list itself is the surface, and breaking it breaks the
  test.

## Status

Termination signal: `NONE`. Verdict: `LOW`. Phase 8 cleared.
