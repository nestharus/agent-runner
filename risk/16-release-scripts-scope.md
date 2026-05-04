# WU-16-01 Scope Risk

Phase: 4 scope-risk gate
Reviewer role: scope-risk judge per
`~/ai/workflows/implementation-pipeline.md` Phase 4
Inputs reviewed:
- Ticket at `tmp/scratch/wu-16-01/ticket.md` (Code Boundary
  `:81-107`, Anti-scope `:123-133`)
- Problem map at `research/16-release-scripts-problem-map.md`
- Proposal at `proposals/16-release-scripts.md`
- WU-13-01 / WU-15-01 precedents at `proposals/13-release-restore.md`
  and `proposals/15-empty-bodies-ref.md`

## Verdict
LOW

## Findings

No BLOCKING or NON-BLOCKING scope findings. The proposal's §1
explicitly locks each of the ticket's four Anti-scope items
(`proposals/16-release-scripts.md:14-22`) and adds proposal-derived
anti-scope (`:26-71`) that further constrains the change to release
CI + install-doc surfaces only. The §6 Implementation outline
(`:513-608`) names exactly the four files in the ticket Code Boundary
and no others. Each ticket Anti-scope clause was probed below; none
triggered.

### Probed clauses (no finding)

- Bare-binary platform-suffix contract (WU-13-01) — proposal §1
  states UNCHANGED with four cite-backed assertions at
  `proposals/16-release-scripts.md:28-37` and the §6 publish-step
  extension explicitly preserves it at `:520-528`. The §4 AC-6 test
  intent at `:456-479` keeps WU-13 bare-binary/bundle assertions
  active. No surface change.
- `.deb` / `.dmg` / `.msi` packaging — proposal §1 states UNCHANGED
  at `:38-41`. §6 trade-off discussion at `:530-544` rejects Option B
  ("copy scripts into `artifacts/`") because it would touch staging,
  and chooses Option A which leaves Tauri bundle wiring untouched.
- Runtime version-skew detection — proposal §1 marks DEFERRED at
  `:61-63`; the README note at §2 `:185-189` describes user-visible
  matched-version guidance only, no runtime check.
- Backwards-compatibility shims for stale-script detection — none
  introduced. Proposal §1 echoes ticket Anti-scope at `:18-22`.
- Modifications to scripts themselves — proposal §1 states UNCHANGED
  at `:42-49`. The §6 outline mentions interpreter shebangs only as
  factual statements about already-shipped files at `:594-602`.
- Code Boundary surface — §6 implementation outline touches:
  `.github/workflows/release.yml` (`:516-528`),
  `src-tauri/tests/release_yml_contract.rs` (`:546-567`),
  `README.md` (`:569-579`), and optional `scripts/README.md`
  (`:581-587`). All four are explicitly in-scope per
  `tmp/scratch/wu-16-01/ticket.md:81-93`. No other file appears as an
  edit target.
- New release job / new packaging step / cross-job artifact rewiring
  — Option A appends entries to the existing
  `softprops/action-gh-release@v2` step's `files:` input. No new
  job, no new staging copy, no change to upload/download artifact
  wiring. Cited at `proposals/16-release-scripts.md:131-148`.
- Auto-PATH installation — proposal §1 marks OUT OF SCOPE at `:64-66`.
- `scripts.tar.gz` bundle alternative — proposal §1 explicitly
  rejects at `:67-71`. Individual files match ticket recommendation.
- AC-1 asset list scope-confusion (i.e., uploading
  `scripts/migrate-model-names.sh`, `scripts/tests/`, or
  `scripts/README.md` as release assets) — §4 AC-1 binds set equality
  over `artifacts/*` plus exactly the seven adapter paths at
  `:351-362`, which structurally rejects an accidental
  `scripts/*` glob.

## Observations

- Strict set-equality assertion shape. §4 AC-1 (`:357-362`) requires
  the structural test to compare `with.files` lines as a
  `BTreeSet<String>` against `{artifacts/*, ...seven paths}`. This
  is stronger than the bare-minimum AC-2 wording in the ticket and
  is a desirable scope guardrail because it fails CI if a future
  edit broadens to `scripts/*` (which would silently include
  `scripts/README.md`, `scripts/tests/`, or
  `scripts/migrate-model-names.sh`). Worth preserving in Phase 5/6.
- Optional `scripts/README.md` cross-reference (`:581-587`) is
  recommended as a one-line pointer to the README install section
  with no duplicated install command. This stays inside the
  ticket's "optional, may add a one-line cross-reference" allowance
  at `tmp/scratch/wu-16-01/ticket.md:91-93`. If Phase 5/6 grows the
  edit beyond a single sentence (e.g., a parallel install command
  block), revisit; the current shape is in scope.
- Windows portability footnote (`:603-608`) describes that scripts
  remain Unix-style as shipped and are not wrapped for Windows.
  This is an explicit non-action — no Windows wrapper, no
  `.ps1`/`.bat`/`.exe` artifacts. Reads as scope-protective rather
  than scope-creeping.
- Residual-risk artifact at `risk/16-release-scripts-test-residuals.md`
  (proposal §4 AC-2/4/5/6, e.g. `:395-397`, `:432-435`) is a Phase
  6b output rather than an in-scope source-code edit. Consistent
  with the pipeline's residuals pattern.
- Trial-release evidence is acknowledged as not guaranteed; AC-2
  residual is properly captured. No scope concern.
- The proposal preserves the existing source-build install snippet
  at `README.md:340-350` and inserts the new binary-install snippet
  immediately after it (`:166-179`, `:571-579`), per AC-3
  (`tmp/scratch/wu-16-01/ticket.md:60-68`). The doc edit stays local
  to one README subsection plus its lead-in to `## Session
  Ingestion`; no doc surface widening.
- The §6 trade-off discussion of Option A vs Option B is design
  reasoning inside the in-scope file, not a second surface. Option
  B is named only to be rejected.

## Status
LOW
