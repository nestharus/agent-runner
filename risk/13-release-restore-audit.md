verdict: LOW

## 1. Verdict

LOW

## 2. Findings

### AUDIT-01

- severity: `low`
- location: `proposals/13-release-restore.md:371`, `proposals/13-release-restore.md:544`, `proposals/13-release-restore.md:573`, `proposals/13-release-restore.md:618`, `proposals/13-release-restore.md:641`, `proposals/13-release-restore.md:666`, `proposals/13-release-restore.md:699`, `proposals/13-release-restore.md:811`
- summary: The proposal contains several self-referential or forward references to risk-report sections that are not stable path:line evidence.
- evidence: The proposal cites `risk report § AUDIT-04` for A3 verification (`proposals/13-release-restore.md:371`), `risk report § SHORT-02` for AC-3 test detail (`proposals/13-release-restore.md:544`), `risk report § AUDIT-03 and § SHORT-03` for AC-4 (`proposals/13-release-restore.md:573`), `risk report § AUDIT-02` for AC-5 (`proposals/13-release-restore.md:618`), `risk report § AUDIT-01 and § SHORT-01` for AC-6 (`proposals/13-release-restore.md:641`, `proposals/13-release-restore.md:666`), `risk report § AUDIT-06` for AC-7 (`proposals/13-release-restore.md:699`), and `risk report § AUDIT-07` for non-interference evidence (`proposals/13-release-restore.md:811`). Those labels are not existing source artifacts in the proposal/problem-map/ticket input set. The audit risk is low because each cited area also includes concrete, independently checkable criteria: A3 names the Phase 6 constructor-invariant evidence (`proposals/13-release-restore.md:361-370`), AC-3 names the portable test file and cross-process assertions (`proposals/13-release-restore.md:521-542`), AC-5 spells out YAML invariants (`proposals/13-release-restore.md:587-617`), AC-6 names required release evidence fields (`proposals/13-release-restore.md:645-665`), and non-interference names exact diff and `rg` checks (`proposals/13-release-restore.md:801-810`).
- closure expectation: Remove or replace the forward risk-report section references with stable proposal section headings or existing path:line citations, while preserving the already-specified verification criteria.

### AUDIT-02

- severity: `info`
- location: `proposals/13-release-restore.md:484-730`
- summary: Every ticket AC is tied to a specific downstream artifact or command with enough detail for Phase 6 and later reviewers to verify closure.
- evidence: AC-1 is mapped to a structural Rust integration test that checks the Windows matrix row and collect-step presence (`proposals/13-release-restore.md:486-497`), while the ticket requires that Windows row and collect step (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:93-97`). AC-2 is mapped to `cargo check --target x86_64-pc-windows-msvc` against the actual manifest/package shape plus grep evidence for Windows-compiled imports (`proposals/13-release-restore.md:499-510`), correcting the ticket's package-name mismatch noted by the problem map (`research/13-release-restore-problem-map.md:100-102`). AC-3 names `src-tauri/tests/session_lock_cross_platform.rs` and the expected acquire/release, busy, token-invalid, idempotent replay, and sibling-process exclusivity assertions (`proposals/13-release-restore.md:512-548`). AC-4 names the Unix Initiative 06 tests and the Windows `cargo check --target x86_64-pc-windows-msvc --tests` evidence split (`proposals/13-release-restore.md:550-578`). AC-7 pins local Rust, frontend, and Windows evidence commands (`proposals/13-release-restore.md:672-701`). AC-8 lists the required D-006 substance and forbidden old framing (`proposals/13-release-restore.md:703-729`).
- closure expectation: No proposal change is required for this item; downstream phases should preserve the named evidence artifacts and command logs.

### AUDIT-03

- severity: `info`
- location: `proposals/13-release-restore.md:587-617`
- summary: The structural `release.yml` test is reproducible at the property level rather than described only as a generic workflow-shape assertion.
- evidence: The proposal requires `jobs.build.strategy.matrix.include` length exactly `3`, exact Linux/macOS/Windows matrix entries, OS-guarded collect steps, target-suffixed bare-binary copy destinations, upload step `name: ${{ matrix.target }}` and `path: artifacts/*`, `download-artifact` with `merge-multiple: true`, release upload `files: artifacts/*`, and bundle globs tied to their target without bare-binary suffixing (`proposals/13-release-restore.md:587-617`). This directly satisfies the ticket's structural-test requirement for the artifact-naming contract (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:169-172`) and the problem map's requirement that a structural test read matrix rows, collect steps, upload path/name, flattened download, and release files (`research/13-release-restore-problem-map.md:383-387`).
- closure expectation: No proposal change is required for this item; Phase 6 should implement the test against these exact invariants or document any intentionally equivalent invariant names.

### AUDIT-04

- severity: `info`
- location: `proposals/13-release-restore.md:625-670`
- summary: AC-6 has concrete gate-pass evidence for a real trial release run, including URLs, artifact inventory, per-platform hashes, matrix listings, and Windows bundle filenames.
- evidence: The ticket requires a trial release run that publishes Linux x86-64, macOS aarch64, and Windows x86-64 binaries, with platform-suffixed bare binaries and conventional bundle names (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:119-122`). The proposal chooses a real GitHub Actions `workflow_dispatch` run over `act` for closure (`proposals/13-release-restore.md:633-640`) and requires `workflow_run_url`, `workflow_run_id`, `release_url`, trial tag, visible release asset inventory, SHA-256 hashes for all three bare binaries, per-target matrix artifact listings, and `windows_bundle_filenames` (`proposals/13-release-restore.md:645-662`). It also states that structural workflow tests and ordinary build logs cannot substitute for this release evidence (`proposals/13-release-restore.md:664-665`).
- closure expectation: No proposal change is required for this item; Phase 6/7/8 closure should require the named AC-6 evidence record rather than narrative confirmation.

### AUDIT-05

- severity: `info`
- location: `proposals/13-release-restore.md:8-68`, `proposals/13-release-restore.md:801-813`
- summary: Cross-WU non-interference is traceable to explicit file boundaries and concrete verification checks.
- evidence: The proposal restates the ticket anti-scope for #36 routing/balancer paths, body-storage/canonical-record/session_turns paths, routing reproduction harnesses, bundle-name constraints, frontend, e2e, and CI workflow boundaries (`proposals/13-release-restore.md:8-68`). The ticket excludes `src-tauri/src/balancer/`, `src-tauri/src/quota/`, `src-tauri/src/state/db.rs`, body-storage work, frontend, and routing harness deletion (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-180`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-196`). The problem map explains the same #36 and body-storage adjacency and cites the exact non-target surfaces (`research/13-release-restore-problem-map.md:296-309`, `research/13-release-restore-problem-map.md:460-475`). The implementation outline requires a final non-interference audit with `git diff --name-only` and targeted `rg` checks proving no edits or new references under the anti-scope surfaces (`proposals/13-release-restore.md:801-810`).
- closure expectation: No proposal change is required for this item; downstream phases should keep the non-interference audit as a named evidence artifact.

### AUDIT-06

- severity: `info`
- location: `proposals/13-release-restore.md:324-482`
- summary: Unconfirmed or partially confirmed assumptions have explicit Phase 6 verification paths.
- evidence: A3 is marked confirmed for constructors but unconfirmed for unusual mount/reparse-point layouts and requires Phase 6 constructor-invariant evidence plus residual-risk recording (`proposals/13-release-restore.md:351-375`), matching the problem map caveat that no device/volume identity check exists (`research/13-release-restore-problem-map.md:411-415`). A4 is marked unconfirmed/currently not applicable and requires a pre-edit `rg` check for hard-link APIs (`proposals/13-release-restore.md:377-390`), matching the problem map's current no-hard-link finding (`research/13-release-restore-problem-map.md:270-275`, `research/13-release-restore-problem-map.md:416-419`). A5 requires generated Windows bundle names to be captured in AC-6 release-run evidence (`proposals/13-release-restore.md:392-414`). A6 is unconfirmed for the future release runner and requires the release build job or Windows-target cargo check (`proposals/13-release-restore.md:416-432`). A7 requires Windows-target `cargo check` and Unix tests after adding `fs4` (`proposals/13-release-restore.md:434-448`). A8 is unconfirmed by automated tests and explicitly routes closure through D-006 documentation plus functional-lock tests, not ACL-layout tests (`proposals/13-release-restore.md:450-466`). A9 requires Phase 6 command mapping for the actual package name (`proposals/13-release-restore.md:468-482`).
- closure expectation: No proposal change is required for this item; Phase 6 evidence must preserve the status and verification result for each assumption.

## 3. Verdict justification

The proposal is auditable, so the verdict is LOW. It gives downstream reviewers concrete AC-to-artifact mappings, exact test file candidates, runnable commands, property-level `release.yml` invariants, release-run evidence fields, and cross-WU non-interference checks tied to ticket and problem-map boundaries. The only audit-risk gap is AUDIT-01: several citations point to future/parallel risk-report section labels rather than stable path:line sources. That is a citation-hygiene issue, not a closure ambiguity, because the proposal itself contains the operative verification criteria beside those references and cites the ticket/problem-map/code surfaces needed to validate them. No medium or high auditability gaps were found.
