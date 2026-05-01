# 06-schema-probe Commit Hygiene
Verdict: PASS-WITH-FINDINGS

One deliberate red exception is present at Step 6b. No other
commit-hygiene issue was found.

## Inputs Reviewed
- `git log --oneline 06-schema-probe ^main`
- `git log 06-schema-probe ^main`
- `~/ai/conventions/git.md`
- `git log --reverse --name-status 06-schema-probe ^main`
- Per-commit compile pass in a temporary detached worktree

## Required Checks
- Agent trailers: PASS.
- Required command returned `0`:
  `git log --pretty=full 06-schema-probe ^main | grep -c "Co-Authored-By: Claude"`.
- Agent generation lines: PASS.
- Direct scan found no `Generated-By`, `Generated with`, `Claude wrote`,
  `agent generated`, or agent authorship trailer.
- A broader agent-name scan only found the domain phrase
  "`Codex schema drift`" in the problem-map message.
- Single concern per commit: PASS.
- Why explained per commit: PASS.
- Fixup noise: PASS.
- `rg -i "fixup|squash|wip|tmp|debug"` against oneline log found no hits.
- Build-alone: PASS-WITH-FINDINGS due the deliberate Step 6b red commit.

## Build-Alone Evidence
Command used at each branch-only commit:
`cargo test --manifest-path src-tauri/Cargo.toml --no-run`.

- `2be658d` problem map: PASS.
- `a1d0f81` Phase 3 proposal: PASS.
- `5b5ed64` Phase 4 Round 1 reports and history: PASS.
- `95f0d1e` Rev 2 proposal closure: PASS.
- `b81bb93` Phase 4 Round 2 closure: PASS.
- `760440d` Phase 5 hookpoints: PASS.
- `be66405` Phase 6 Step 6a contract: PASS.
- `7bdc4ee` Phase 6 Step 6b tests: FAIL, deliberate red exception.
- `3385efb` Phase 6 Step 6c implementation: PASS.

The Step 6b failure matches its commit body. The commit says it
compile-fails until Step 6c provides `StateDb::open_read_only`,
`ReadOnlyOpenError`, and `SchemaProbeReport`; compiler output showed
unresolved import/API errors for that Step 6c surface.

## Per-Commit Hygiene
- `2be658d` `research`: single concern is touched-surface inventory.
  Why is explained by identifying mutation behavior, schema-version absence,
  JSON precedent, and fresh-install vs post-migration probes.
- `a1d0f81` `plan`: single concern is the Phase 3 proposal.
  Why is explained by D1-D7 decisions and the move to Phase 4.
- `5b5ed64` `risk`: single concern is Round 1 reports plus audit history.
  Why is explained by MEDIUM findings, LOW companion reports, and continue
  decision requiring Rev 2.
- `95f0d1e` `plan`: single concern is Rev 2 proposal closure.
  Why is explained by R1-F01 compatibility-map shape closure and R1-F02
  `ReadOnlyOpenError` variant closure.
- `b81bb93` `risk`: single concern is Round 2 report updates.
  Why is explained by LOW x 4 verdicts and clearance to hookpoint research.
- `760440d` `research`: single concern is the Phase 5 hookpoint map.
  Why is explained by mapping all 13 proposal sections to file:line targets.
- `be66405` `contract`: single concern is the Step 6a implementation contract.
  Why is explained by bridging Rev 2 to CLI, API, JSON, exits, side effects,
  fixtures, and process-tree obligations.
- `7bdc4ee` `test`: single concern is Step 6b tests and fixtures.
  Why is explained by contract-driven T1-T8 coverage before implementation.
  This is the accepted red exception and is explicitly documented as such.
- `3385efb` `feat`: single concern is Step 6c implementation with necessary
  test fixture/test adjustments.
  Why is explained by introducing schema-probe, read-only DB open, schema
  report, build commit injection, and passing test evidence.

## Decision
The branch satisfies the commit hygiene requirements with one accepted
exception: Step 6b is intentionally red to preserve test-before-code
separation. There are no agent trailers, no generation lines, no fixup
commits, and no unexplained mixed-concern commits.
