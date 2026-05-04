# WU-16-01 Supported-Surface Risk

Phase: 4 supported-surface risk gate
Work unit: `release-scripts`
Subject under review: `proposals/16-release-scripts.md`
Inputs cross-checked: `research/16-release-scripts-problem-map.md`,
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md`,
`risk/13-release-restore-supported-surface.md`,
`risk/15-empty-bodies-ref-supported-surface.md`.

## Termination signal

`NONE`.

### Assumption invalidation check

A1..A6 in the proposal's approved register
(`proposals/16-release-scripts.md:221-332`) all stand against the current
HEAD `bc6df8e fix(state): persist session turn bodies in state.db (#40)`:

- **A1 (softprops `with.files` accepts a newline-delimited list).** Not
  invalidated. The publish step at
  `.github/workflows/release.yml:177-181` already binds `files:
  artifacts/*` as a YAML scalar input to the v2 action; the upstream
  metadata exposes `files` as a single string input that the action
  splits by newline before globbing. Switching the scalar to a YAML
  block scalar with `artifacts/*` plus seven explicit `scripts/<name>`
  lines stays inside the documented input shape.
- **A2 (direct `scripts/<name>` upload preserves basename as asset
  name).** Not invalidated. The current bare-binary upload pattern at
  `.github/workflows/release.yml:139-158` already relies on the same
  basename behavior — staged files are listed via `artifacts/*` and
  appear on the release page under their last-segment name. AC-1's
  patterns at `tmp/scratch/wu-16-01/ticket.md:42-53` and the README
  `gh release download --pattern <bare-name>` lines confirm bare-name
  expectations.
- **A3 (seven adapter scripts exist under `scripts/`).** Not
  invalidated. Verified at HEAD: `scripts/` contains
  `anthropic-usage`, `chatgpt-usage`, `claude-code-locate-transcript`,
  `claude-code-turns`, `codex-locate-transcript`, `codex-turns`, and
  `zai-usage`, plus the out-of-scope `migrate-model-names.sh`,
  `README.md`, and `tests/` per AC-1's exclusion list.
- **A4 (structural test extends without rewrite).** Not invalidated.
  `src-tauri/tests/release_yml_contract.rs:253-262` still uses
  `step_by_uses(release_steps, "softprops/action-gh-release@v2")`
  followed by `string_at(.., &["with", "files"])` and equality against
  `"artifacts/*"`. Helpers at `:320-377` remain available, and a YAML
  block scalar still parses as `Value::String` under `serde_yml`.
- **A5 (README has stable insertion point adjacent to source-build
  snippet).** Not invalidated. `README.md:332-350` still hosts the
  `**Reference quota adapters**` table and the
  `install -m 755 scripts/... ~/.local/bin/` snippet, and
  `## Session Ingestion` still follows at `README.md:352`. The
  source-build snippet preserves the same five names listed in the
  problem map, including the historical absence of the two
  transcript-locator entries — this is acknowledged in the problem map
  as a documentation gap that AC-3 closes.
- **A6 (`scripts/README.md` accepts a one-line cross-reference).** Not
  invalidated. `scripts/README.md:1-5` still describes adapters as
  standalone executables wired through TOML, and the ticket marks the
  file optional at `tmp/scratch/wu-16-01/ticket.md:91-93`.

### Net-value check

Positive. The proposal closes one concrete current-state failure on
the supported install surface:

- RC-1: binary-install users who installed v0.1.26 receive a body-aware
  `oulipoly-agent-runner` binary but no matching adapter scripts,
  leaving `session_turns.body` NULL until they manually copy scripts
  from a repo clone (`tmp/scratch/wu-16-01/ticket.md:8-30`;
  `.github/workflows/release.yml:177-181` shows the publish step
  uploads only `artifacts/*` today).

The blast radius is bounded: a single YAML edit point at the publish
step, one structural-test extension at the existing `gh_release`
lookup, one README snippet adjacent to the source-build install lines,
and one optional one-line cross-reference in `scripts/README.md`. No
runtime, schema, package-bundle, or frontend code is touched. The
WU-13-01 bare-binary platform-suffix contract and the
`.deb`/`.dmg`/`.msi` bundle contracts remain covered by the test
assertions at `src-tauri/tests/release_yml_contract.rs:14-273`, which
the proposal extends rather than replaces.

Rollback is symmetric: revert the YAML edit, revert the test
extension, revert the README and `scripts/README.md` lines. There is
no DB migration, no installed package state, and no released-asset
state to unwind beyond the single tag the change first ships under.

## Verdict

`LOW`.

## Findings

### SS-01 — Source-build snippet currently omits transcript locators (AC-3 closure)

- Severity: NON-BLOCKING.
- Statement: AC-3 says the existing `§Reference quota adapters`
  source-build snippet "remains valid for source-builds." The current
  snippet at `README.md:340-350` installs five scripts
  (`anthropic-usage`, `chatgpt-usage`, `zai-usage`,
  `claude-code-turns`, `codex-turns`) and does NOT install
  `claude-code-locate-transcript` or `codex-locate-transcript`. The
  binary-install snippet the proposal adds (§2 of the proposal,
  `proposals/16-release-scripts.md:170-179`) downloads all seven AC-1
  scripts including both locators, so source-build users following the
  unchanged snippet would end up with five-of-seven scripts while
  binary-install users get all seven.
- Evidence: `README.md:340-350` (source-build snippet, five scripts);
  `proposals/16-release-scripts.md:170-179` (binary-install snippet,
  seven scripts); ticket AC-1 list at
  `tmp/scratch/wu-16-01/ticket.md:42-53`; problem map adjacency-risk
  note at `research/16-release-scripts-problem-map.md:330-336`.
- Closure expectation: this is not a Phase 4 termination because AC-3
  literally requires only that the source-build snippet "remain valid"
  (i.e., compile and install the scripts it lists). Phase 5 should
  decide whether AC-3 implementation also broadens the source-build
  snippet to the same seven-script set or accepts the documented
  delta. Either path keeps the supported surface positive; the gate
  records this so Phase 5 does not ship asymmetric install shapes
  silently.

### SS-02 — `softprops/action-gh-release@v2` block-scalar parse shape (A1/A4 closure)

- Severity: NON-BLOCKING.
- Statement: A1 commits the publish step to a YAML block scalar over
  `with.files` containing `artifacts/*` plus the seven script paths.
  The structural test at `src-tauri/tests/release_yml_contract.rs:253-262`
  currently uses `string_at(.., &["with", "files"])` and compares
  against `"artifacts/*"`. `serde_yml` does parse a block scalar as
  `Value::String`, so the helper continues to work, but the new
  assertion has to split on `\n`, trim, and filter empties before set
  comparison — a mechanically different shape from today's plain
  equality check. This was identified by Round 2 as the AUDIT-01
  closure (`proposals/16-release-scripts.md:647-658`) but the exact
  helper shape is left to Phase 5.
- Evidence: current scalar assertion at
  `src-tauri/tests/release_yml_contract.rs:253-262`; helper functions
  at `src-tauri/tests/release_yml_contract.rs:327-346`; proposal §6
  structural-test extension at
  `proposals/16-release-scripts.md:546-567`; proposal Round-2 changelog
  at `proposals/16-release-scripts.md:649-658`; proposal open question
  1 at `proposals/16-release-scripts.md:611-617`.
- Closure expectation: Phase 5 hookpoint research binds the exact
  helper shape (inline split-trim-filter into `BTreeSet<String>` vs. a
  small local helper). Phase 6b compiles and runs the test against the
  edited workflow. No proposal-level revision is required.

### SS-03 — Live release-asset materialization not structurally provable

- Severity: NON-BLOCKING.
- Statement: AC-2 has two halves: a structural test extension and a
  trial release publishing all expected scripts. The structural test
  proves workflow shape but cannot prove that GitHub actually
  produced asset entries on a real tag. AC-4 and AC-6 inherit the same
  live-vs-structural split. The proposal already names the residual
  artifact `risk/16-release-scripts-test-residuals.md` as the closure
  channel if no trial release is performed.
- Evidence: AC-2 evidence schema at
  `proposals/16-release-scripts.md:368-397`; AC-4/AC-6 residuals at
  `proposals/16-release-scripts.md:432-435`, `:473-479`; Round-2
  closure of AUDIT-02 at `proposals/16-release-scripts.md:664-666`;
  WU-13 precedent for separating structural and release-run evidence
  at `risk/13-release-restore-supported-surface.md:426-445`.
- Closure expectation: if Phase 6b runs no trial release, it must
  write `risk/16-release-scripts-test-residuals.md` describing the
  unverified live-release path and the structural coverage that
  remains. The gate does not escalate because the proposal commits the
  residual-risk path explicitly.

### SS-04 — Windows users not first-class consumers of these scripts

- Severity: NON-BLOCKING.
- Statement: All seven scripts are POSIX shell or Python with
  `#!/usr/bin/env` shebangs and `chmod +x`-based install. The README
  binary-install snippet at `proposals/16-release-scripts.md:170-179`
  uses `gh release download --dir ~/.local/bin/` and `chmod +x`, which
  is a Linux/macOS path. The proposal acknowledges this at
  `proposals/16-release-scripts.md:589-608` and the ticket explicitly
  excludes Windows-specific script wrappers. Windows users still
  receive a working bare `oulipoly-agent-runner.exe` and Tauri bundles
  (the WU-13-01 contract is preserved), but the body-ingestion path
  through these scripts assumes a POSIX-like shell plus Python.
- Evidence: shebangs at `scripts/claude-code-turns:1-4`,
  `scripts/codex-turns:1-5`, `scripts/anthropic-usage:1-5`,
  `scripts/chatgpt-usage:1-5`, `scripts/zai-usage:1-5`,
  `scripts/claude-code-locate-transcript:1-5`,
  `scripts/codex-locate-transcript:1-5`; proposal Windows note at
  `proposals/16-release-scripts.md:605-608`; ticket out-of-scope at
  `tmp/scratch/wu-16-01/ticket.md:95-107`.
- Closure expectation: none at gate level. Windows script portability
  is the script-set's existing v1 stance; this WU is a release-asset
  delivery change, not a portability change. Phase 5/6b inherit no new
  obligation here. Recorded so reviewers do not read AC-3's "binary
  install" framing as a Windows-install guarantee.

### SS-05 — Optional `scripts/README.md` cross-reference is opt-in only

- Severity: NON-BLOCKING.
- Statement: A6 and the proposal's open question 3 leave the
  optional `scripts/README.md` cross-reference up to Phase 5. The
  ticket Code Boundary marks this file optional. The proposal's
  preference is to add a single sentence pointing at README's
  `§Reference quota adapters`. The risk is purely documentation
  coupling: if Phase 5 declines, AC-3 still closes through README;
  if Phase 5 accepts, the wording must avoid creating a second
  install procedure.
- Evidence: ticket Code Boundary at
  `tmp/scratch/wu-16-01/ticket.md:91-93`; A6 evidence at
  `proposals/16-release-scripts.md:318-332`; proposal §6 cross-ref
  shape at `proposals/16-release-scripts.md:581-587`; open question 3
  at `proposals/16-release-scripts.md:626-631`.
- Closure expectation: Phase 5 decides include/exclude. If included,
  the wording stays as the proposal's prescribed single sentence.
  Either choice keeps `LOW`.

## Observations

- The supported-surface scope is correctly limited to the release-CI
  flow and the binary-install user flow. Runtime, schema, ingest
  reader/writer, quota, routing, frontend, Tauri bundle config, and
  package-installer install paths are all explicitly out of scope and
  the proposal cites the ticket's exclusions inline at multiple
  points.
- The proposal's chosen edit shape (Option A: append explicit
  `scripts/<name>` paths to `with.files`) matches the WU-13-01
  precedent of structurally visible release-asset paths
  (`risk/13-release-restore-supported-surface.md:312-329`). It avoids
  the alternative of staging non-build repo files into `artifacts/`,
  which would have widened the staging surface and weakened the
  structural assertion.
- The proposal's anti-scope §1 is verbatim faithful to the ticket
  anti-scope and adds proposal-derived items (no scripts.tar.gz, no
  modification of the seven scripts, no platform-suffix changes, no
  Tauri-bundle script embedding) that all map to ticket clauses.
- The qualitative net-value statement at
  `proposals/16-release-scripts.md:487-511` names the concrete user
  cohort (binary-install users on v0.1.26+) and the concrete loss
  avoided (`session_turns.body` silently empty until manual repo
  copy), with the cost itemized as four edit points. Comparable in
  shape to the WU-13-01 net-value structure
  (`risk/13-release-restore-supported-surface.md:208-263`) and the
  WU-15-01 net-value structure
  (`risk/15-empty-bodies-ref-supported-surface.md:49-67`).
- Rollback symmetry is honest: unlike WU-15-01, this WU has no
  persistent state (no migration, no schema column) so the rollback
  truly is "revert two commits." The proposal's rollback path at
  `proposals/16-release-scripts.md:198-206` matches.
- No assumption is invalidated, no hotspot resolves to `unsound`, and
  the four findings above are NON-BLOCKING contract-/evidence-phase
  obligations rather than proposal-level revisions. Phase 4 clears.

## Status

Termination signal: `NONE`. Verdict: `LOW`. Phase 4 cleared.
