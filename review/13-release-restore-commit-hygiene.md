# Commit Hygiene Audit: WU-13-01 Release Restore

**Branch:** `impl/wu-13-01`  
**Base:** `main` at `6b9509e`  
**Commit reviewed:** `bff6a69` - `fix(release): restore Windows port + per-platform bare-binary names`  
**Mode:** audit-only; branch not rewritten or pushed  
**Verdict:** PASS  
**Risk verdict:** LOW

## Summary

The single review commit is acceptable for this gate. It is large
(`16 files changed, 4219 insertions, 87 deletions`) and contains both
release-pipeline and lock-portability work, but it is not a vague
mega-commit: the title names both shipped behaviors, the body explains
why each grouped section exists, and the changed files are separated
well enough for a reviewer to read the commit concern-by-concern.

No history rewrite is recommended. A split into many process/document
commits would add mechanical review overhead without improving the
behavioral story for this WU.

## Gate Checks

- **Title:** PASS. The subject is Conventional-Commits-shaped,
  scoped to `release`, specific, and 67 characters long.
- **Body:** PASS. The body explains the why for the grouped changes:
  Windows support restoration after #24, bare-binary asset collision
  avoidance for #22, fs4 selection for cross-platform locking, the
  Windows-default ACL choice, contract-test coverage, and residual
  risk around hard-link semantics.
- **Diff readability:** PASS. The body bullets map directly to file
  groups:
  - release workflow and artifact naming:
    `.github/workflows/release.yml`
  - lock portability:
    `src-tauri/src/session_lock/mod.rs`, `src-tauri/Cargo.toml`,
    `src-tauri/Cargo.lock`
  - executable contracts:
    `src-tauri/tests/release_yml_contract.rs`,
    `src-tauri/tests/session_lock_cross_platform.rs`
  - decision, proposal, research, risk, and contract artifacts:
    `DECISIONS.md`, `proposals/`, `research/`, `risk/`,
    `product-strategy/contracts/`
- **Co-author hygiene:** PASS. The commit has one explicit
  `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>` trailer.
  Author and committer are both `nestharus <contact@nestharus.com>`.
  No co-author drift is visible in the single-commit branch metadata.
- **Drop-then-restore:** PASS. The branch has one commit, so there is
  no cross-commit transient removal/restoration pattern to detect.

## Per-Commit Evaluation

### `bff6a69` - `fix(release): restore Windows port + per-platform bare-binary names`

- **Classification:** release-restoration fix with paired tests,
  decision record, proposal/research/risk artifacts, and contract
  documentation.
- **Concern count:** two declared behavior concerns within one WU:
  Windows release support and target-suffixed bare binary names. The
  session-lock portability work is necessary support for the Windows
  release row; the release workflow change is the user-visible release
  restoration. The tests and docs are paired with those behaviors.
- **Message score:** high. The subject is concise and the body is
  detailed, structured, and rationale-oriented.
- **Anti-patterns found:** none of the commit-organization blockers
  apply. This is not a `wip`/`fix`/`address feedback` message, not a
  CodeRabbit catch-all commit, and not a refactor disguised as a
  behavior change. The fs4 replacement is a behavior-supporting port,
  not a pure refactor.
- **Tests pass:** yes for the scoped contract tests run during this
  audit.

## Verification

Commands run:

```bash
git status --short --branch
git log --oneline --decorate main..impl/wu-13-01
git show --stat --format=fuller bff6a69
git show -s --format=%B bff6a69
git diff --name-status main..impl/wu-13-01
git show --check --format=short bff6a69
git diff --check main..impl/wu-13-01
cargo test --manifest-path src-tauri/Cargo.toml --test session_lock_cross_platform -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --test release_yml_contract -- --nocapture
```

Results:

- Worktree was clean on `impl/wu-13-01`.
- Branch contains exactly one commit over `main`.
- `session_lock_cross_platform` passed: 4 passed, 1 ignored helper
  test, 0 failed.
- `release_yml_contract` passed: 1 passed, 0 failed.
- `git diff --check` reports two trailing-whitespace lines in
  `proposals/13-release-restore.md`:
  - line 3: `Phase: 3 proposal  `
  - line 4: `Work unit: \`release-restore\`  `

The trailing whitespace is a style cleanup note, not a commit-history
organization failure for this gate. It should be fixed before merge if
the repository treats `git diff --check` as a blocking quality check.

## Recommendation

Do not reorganize the branch history. The single commit is reviewable
as-is because the message and file layout preserve the WU's internal
structure, the scoped tests pass, and no co-author or drop-restore
hygiene issue is present.
